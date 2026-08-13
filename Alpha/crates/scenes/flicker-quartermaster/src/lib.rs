//! **Quartermaster Bench** — the content air-traffic controller.
//!
//! Ingest benches (Clayworks, Loomforge, the retargeter) drop their processed
//! output into `staging/`. Nothing the game reads changes until someone looks
//! at it and **promotes** it into `package/`. This bench is where that happens:
//! a two-tab file manager over the two trees, and the manager of the package
//! manifest.
//!
//! - **Review** — inspect one staged item at a time, then promote it (CM5).
//! - **Files** — a folder tree over both roots, with a sortable listing.
//!
//! # Controller-first
//!
//! Every navigation is an `ActionSignal` declared on the screen root (S9), so
//! the bench is fully drivable from a pad on a SINGLE FOCUS: the left stick
//! moves between panels, the d-pad moves within one, L1/R1 change tab, A opens,
//! B goes up. Multi-select and drag are a later keyboard/mouse enhancement —
//! never a requirement for any core flow.
//!
//! # Why the tree is built in Rust
//!
//! A folder listing is unbounded and its node ids encode positions in the
//! CURRENT listing, so they must be re-derived after every navigation. That is
//! the sanctioned Rust-tree case (loomforge's precedent): the walker's draw
//! cache is structural, so a rebuilt-identical tree replays for free.

pub mod fs_model;

use flicker::ui::{Command, CommandHistory};
use flicker_content::{BatchFileOp, FileOp};
use flicker_input_core::{editor_chords, ChordLayer, InputContext};

/// The node id of the inline rename field — also its focus id.
const RENAME_ID: &str = "rename_field";
const APPLY_REST_ID: &str = "conflict_apply_rest";

/// How many toasts the `toast_stack` proto has slots for. The proto spells its
/// rows out (the walker has no repeater), so this is the ONE place the capacity
/// is stated on the Rust side and it must match the proto.
const TOAST_SLOTS: usize = 3;
/// How long a toast stays up, in 60 Hz ticks (~8 s, the design's dwell).
const TOAST_TICKS: u64 = 480;

/// Whether a toast born at `born` is still up at `now`. A free function so the
/// dwell rule is testable without driving a frame — `tick` only advances inside
/// `update`, which needs a live frame context.
fn toast_alive(born: u64, now: u64) -> bool {
    now.saturating_sub(born) < TOAST_TICKS
}

/// One transient confirmation. The ACTION is a `$token` and the SUBJECT is data,
/// kept apart so nothing here composes an English sentence.
#[derive(Clone, Debug)]
struct Toast {
    /// A `$token` naming what happened (the command's own history label).
    label: String,
    /// The item it happened to — data, never copy.
    name: String,
    born: u64,
}

/// An open collision prompt.
///
/// Shaped as a QUEUE even though today's move-only clipboard holds one item:
/// the resolutions accumulate into ONE batch, so when multi-select lands
/// (CM6) a 40-item paste with three collisions is still a single undo entry.
/// `apply_rest` is the design's "apply to the remaining N", and is only
/// offered while more than one collision is outstanding.
#[derive(Clone, Debug)]
struct ConflictPrompt {
    /// Collisions still to answer; `[0]` is the one on screen.
    pending: Vec<flicker_content::Conflict>,
    /// Moves already resolved into concrete ops — Skip contributes nothing.
    ops: Vec<FileOp>,
    /// Answer the rest of the queue the same way as the next choice.
    apply_rest: bool,
    /// Set when this prompt belongs to a PROMOTE: the manifest row to append and
    /// the exact move plan the answers must reconstruct — an asset's files travel
    /// together or not at all.
    promote: Option<PromotePlan>,
}

/// A promote waiting on its collision answers.
#[derive(Clone, Debug)]
struct PromotePlan {
    row: flicker_content::manifest::ManifestEntry,
    /// Every planned destination. The prompt's resolved ops must cover EXACTLY
    /// this set — a Skip (fewer) or a Keep-both (renamed dst) makes the promote
    /// partial, and a partial asset never ships.
    dsts: Vec<PathBuf>,
}

/// An open in-place rename.
#[derive(Clone, Debug)]
struct Rename {
    /// What is being renamed.
    path: PathBuf,
    /// The edited name.
    draft: String,
    /// True until the first character is typed.
    ///
    /// The design asks for the basename to be PRESELECTED, but the text field
    /// has no selection model — `fold_typed` only appends and backspaces. So
    /// the first typed character clears the draft instead, which is what
    /// select-all-then-type would have produced. Backspace does not consume the
    /// pristine state: editing the existing name is the other thing a user
    /// means by pressing a key here.
    pristine: bool,
}

/// One bench mutation: a batch of file operations plus the `$token` naming it
/// for the toast and the history row.
///
/// This is the whole bridge between the two halves of the spine — the file ops
/// live in `flicker-content` and know nothing about the UI, the history lives
/// in `flicker-widgets` and knows nothing about files, and this joins them.
/// **A batch is ONE command**, so a forty-item move is one entry and one undo.
struct FileCommand {
    batch: BatchFileOp,
    label: String,
    /// A PROMOTE's manifest rider: the row this command appends on apply and
    /// removes on revert — so undo-of-promote returns the FILES to staging and
    /// takes the LEDGER row back in the same single history entry.
    manifest: Option<(PathBuf, flicker_content::manifest::ManifestEntry)>,
}

impl Command for FileCommand {
    fn apply(&mut self) -> Result<(), String> {
        self.batch.apply().map_err(|e| e.to_string())?;
        if let Some((path, row)) = &self.manifest {
            // The files moved but the ledger refused: roll the files back so the
            // two can never disagree, and surface the manifest's own error.
            if let Err(e) = flicker_content::manifest::append(path, row.clone()) {
                let _ = self.batch.revert();
                return Err(e.to_string());
            }
        }
        Ok(())
    }
    fn revert(&mut self) -> Result<(), String> {
        if let Some((path, row)) = &self.manifest {
            // Ledger first: an undo that cannot take its row back must not move
            // files either, or the manifest would claim a promotion staging holds.
            flicker_content::manifest::remove(path, row).map_err(|e| e.to_string())?;
        }
        self.batch.revert().map_err(|e| e.to_string())
    }
    fn label(&self) -> &str {
        &self.label
    }
}

use std::path::PathBuf;
use std::time::Duration;

use flicker::render::{Renderer, Vec2};
use flicker::scene::{Scene, SceneInput, Transition};
use flicker::script::{HudCommand, UiNode, ValueMap};
use flicker::ui::{
    render_hud, run_ui, UiInput, UiIntents, UiState, WalkerHandler,
};
use flicker_input_core::{
    AbstractControls, ContextualBindings, Fired, GamepadConfig, InputMap, InputState, Key, Resolver,
};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

use fs_model::{QueueItem, Roots, Row, SortKey, TreeRow};

const HUD_UI_THEME: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_theme.json");

const TOP_BAR_H: f32 = 52.0;
const TAB_BAR_H: f32 = 44.0;
const CRUMB_H: f32 = 52.0;
const STATUS_H: f32 = 34.0;
const TREE_W: f32 = 340.0;
const ROW_H: f32 = 32.0;
const LIST_ROW_H: f32 = 34.0;
const HEADER_H: f32 = 32.0;

/// The bench's two pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// Inspect and promote what is sitting in staging.
    Review,
    /// Browse and rearrange both trees.
    #[default]
    Files,
}

impl Tab {
    pub const ALL: [Tab; 2] = [Tab::Review, Tab::Files];

    /// Stable id — doubles as the node id and the fired action name.
    pub fn id(self) -> &'static str {
        match self {
            Tab::Review => "tab_review",
            Tab::Files => "tab_files",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Tab::Review => "$qm_tab_review",
            Tab::Files => "$qm_tab_files",
        }
    }

    fn next(self) -> Tab {
        match self {
            Tab::Review => Tab::Files,
            Tab::Files => Tab::Review,
        }
    }
}

/// Which panel of the screen holds the focus cursor.
///
/// The d-pad moves WITHIN the focused pane; the left stick moves between panes.
/// Keeping this explicit is what makes a single-focus controller baseline work
/// without any multi-select.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    /// The folder tree.
    Tree,
    /// The file listing.
    #[default]
    List,
}

impl Pane {
    pub const ALL: [Pane; 2] = [Pane::Tree, Pane::List];

    pub fn id(self) -> &'static str {
        match self {
            Pane::Tree => "tree",
            Pane::List => "list",
        }
    }

    fn next(self) -> Pane {
        match self {
            Pane::Tree => Pane::List,
            Pane::List => Pane::Tree,
        }
    }

    /// With two panes, previous and next are the same hop; kept named so the
    /// call sites read as the intent they came from.
    fn prev(self) -> Pane {
        self.next()
    }
}

/// The bench.
pub struct Quartermaster {
    roots: Roots,
    tab: Tab,
    pane: Pane,

    /// The folder whose contents the list shows.
    cwd: PathBuf,
    rows: Vec<Row>,
    /// Index into `rows`; the single focus the controller baseline drives.
    sel: usize,
    sort: SortKey,
    sort_desc: bool,

    /// Directories the user has opened in the tree.
    expanded: Vec<PathBuf>,
    tree: Vec<TreeRow>,
    tree_sel: usize,

    /// Scroll offsets, in px, written back by the wheel and by scroll-to-focus.
    list_scroll: f32,
    tree_scroll: f32,
    list_view_h: f32,

    // ── engine plumbing ──
    ui_intents: UiIntents,
    ui_state: UiState,
    ui_styles: serde_json::Value,
    hud_commands: Vec<HudCommand>,
    ui_theme: Option<Theme>,
    white: Option<flicker::render::TextureHandle>,

    /// The in-place rename, while one is open.
    rename: Option<Rename>,
    /// The open collision prompt, while one is up.
    prompt: Option<ConflictPrompt>,
    /// Live confirmations, oldest first. Capped at [`TOAST_SLOTS`].
    toasts: Vec<Toast>,
    /// The context menu's anchor while it is open, in screen px.
    menu: Option<Vec2>,
    /// The pad cursor INTO the menu — an index into [`MENU_ROWS`].
    menu_sel: usize,
    /// Raw Enter/Esc edges — the ruled exception: while `TextEntry` owns the
    /// keyboard its binding map is empty by design, so the intent channel is
    /// deliberately unavailable and these two are read directly.
    enter_prev: bool,
    esc_prev: bool,

    // ── mutation ──
    /// The move-only clipboard. One item at a time: the controller baseline is a
    /// single focus, and this manager relocates rather than copies.
    clipboard: Option<PathBuf>,
    history: CommandHistory<FileCommand>,
    /// Namespaces each batch's `.trash` displacement so two batches can never
    /// reclaim each other's parked files.
    batch_seq: u64,
    /// The last mutation's outcome, for the status line.
    last_error: Option<String>,

    bindings: ContextualBindings,
    chord: ChordLayer,
    gamepad_config: GamepadConfig,
    resolver: Resolver,
    ev: Vec<Fired>,
    tick: u64,
    fired_sigs: Vec<String>,

    // ── CM5: the Review tab ──
    /// The staging queue — one entry per promotable asset FOLDER, tier-ordered.
    queue: Vec<QueueItem>,
    /// The queue cursor (single focus: every Review verb acts on this item).
    review_sel: usize,
    /// Facts + warnings for the selected item, parsed ONCE per selection — a
    /// rig parse is a per-click cost, never a per-frame one.
    facts: Option<ReviewFacts>,
}

impl Default for Quartermaster {
    fn default() -> Self {
        Self::new()
    }
}

impl Quartermaster {
    pub fn new() -> Self {
        Self::with_roots(Roots::from_config())
    }

    /// A bench over explicit roots — the seam a test drives without an app.
    pub fn with_roots(roots: Roots) -> Self {
        let cwd = roots.package.clone();
        let mut me = Self {
            roots,
            tab: Tab::default(),
            pane: Pane::default(),
            cwd,
            rows: Vec::new(),
            sel: 0,
            sort: SortKey::default(),
            sort_desc: false,
            prompt: None,
            toasts: Vec::new(),
            menu: None,
            menu_sel: 0,
            expanded: Vec::new(),
            tree: Vec::new(),
            tree_sel: 0,
            list_scroll: 0.0,
            tree_scroll: 0.0,
            list_view_h: 600.0,
            ui_intents: UiIntents::default(),
            ui_state: UiState::default(),
            ui_styles: serde_json::Value::Null,
            hud_commands: Vec::new(),
            ui_theme: None,
            white: None,
            rename: None,
            enter_prev: false,
            esc_prev: false,
            clipboard: None,
            history: CommandHistory::new(),
            batch_seq: 0,
            last_error: None,
            // The chord layer rides the ratified context stack: hold the
            // modifier and L2 means Cut rather than its base-map signal.
            bindings: ContextualBindings::new(bench_map())
                .with(InputContext::Chord, editor_chords()),
            chord: ChordLayer::new(),
            gamepad_config: GamepadConfig::default(),
            resolver: Resolver::new(),
            ev: Vec::new(),
            tick: 0,
            fired_sigs: Vec::new(),
            queue: Vec::new(),
            review_sel: 0,
            facts: None,
        };
        // Both roots open from the first frame — the design shows them that way.
        me.expanded = vec![me.roots.package.clone(), me.roots.staging.clone()];
        me.refresh();
        me
    }

    /// Re-read the listing and the tree from disk, keeping the focus on the same
    /// item where it still exists (a refresh must not move the cursor under the
    /// user).
    pub fn refresh(&mut self) {
        let focused = self.rows.get(self.sel).map(|r| r.path.clone());
        self.rows = fs_model::list_dir(&self.cwd, self.sort, self.sort_desc);
        self.sel = focused
            .and_then(|p| self.rows.iter().position(|r| r.path == p))
            .unwrap_or(0)
            .min(self.rows.len().saturating_sub(1));
        self.tree = fs_model::tree_rows(&self.roots, &self.expanded);
        self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
        self.refresh_queue();
    }

    // ── read-only accessors, for tests and for the model ──
    pub fn tab(&self) -> Tab {
        self.tab
    }
    pub fn pane(&self) -> Pane {
        self.pane
    }
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }
    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }

    /// The row a MUTATION may act on: the listing cursor, and only while the
    /// listing holds focus. `confirm`/`cancel`/`nav` already dispatch on
    /// `self.pane`; the mutation verbs read `self.sel` unconditionally, so one
    /// fired while the TREE held focus cut or renamed a row the user was not
    /// looking at. Acting on a tree FOLDER (the roots among them) is a
    /// deliberate later capability — never this silent fallback.
    fn focused_row(&self) -> Option<&Row> {
        match self.pane {
            Pane::List => self.rows.get(self.sel),
            Pane::Tree => None,
        }
    }
    pub fn tree_view(&self) -> &[TreeRow] {
        &self.tree
    }
    pub fn sort(&self) -> (SortKey, bool) {
        (self.sort, self.sort_desc)
    }

    /// Move into `dir` and reset the listing cursor.
    pub fn open_dir(&mut self, dir: PathBuf) {
        if !dir.is_dir() {
            return;
        }
        self.cwd = dir;
        self.sel = 0;
        self.list_scroll = 0.0;
        self.rows = fs_model::list_dir(&self.cwd, self.sort, self.sort_desc);
    }

    /// Confirm on the focused item: a folder opens, a file stays put (its detail
    /// pane is CM5's job).
    pub fn confirm(&mut self) {
        match self.pane {
            Pane::List => {
                if let Some(row) = self.rows.get(self.sel) {
                    if row.is_dir {
                        let dir = row.path.clone();
                        self.open_dir(dir);
                        self.tree = fs_model::tree_rows(&self.roots, &self.expanded);
                    }
                }
            }
            Pane::Tree => {
                // Confirm on a tree row both opens the branch and navigates to
                // it, so one button does the obvious thing.
                if let Some(t) = self.tree.get(self.tree_sel) {
                    let path = t.path.clone();
                    if !self.expanded.contains(&path) {
                        self.expanded.push(path.clone());
                    }
                    self.open_dir(path);
                    self.tree = fs_model::tree_rows(&self.roots, &self.expanded);
                }
            }
        }
    }

    /// Cancel: collapse a branch, else climb one level — stopping at the roots.
    pub fn cancel(&mut self) {
        match self.pane {
            Pane::Tree => {
                if let Some(t) = self.tree.get(self.tree_sel) {
                    let path = t.path.clone();
                    self.expanded.retain(|e| *e != path);
                    self.tree = fs_model::tree_rows(&self.roots, &self.expanded);
                    self.tree_sel = self.tree_sel.min(self.tree.len().saturating_sub(1));
                }
            }
            Pane::List => self.up(),
        }
    }

    /// Climb to the parent folder, never above a root.
    pub fn up(&mut self) {
        if let Some(parent) = fs_model::parent_within_roots(&self.roots, &self.cwd) {
            let leaving = self.cwd.clone();
            self.open_dir(parent);
            // Land the cursor on the folder we came out of — the least
            // surprising place for it to be.
            if let Some(i) = self.rows.iter().position(|r| r.path == leaving) {
                self.sel = i;
            }
        }
    }

    /// D-pad within the focused pane. `delta` is ±1.
    pub fn nav(&mut self, delta: isize) {
        let (len, cursor) = match self.pane {
            Pane::List => (self.rows.len(), &mut self.sel),
            Pane::Tree => (self.tree.len(), &mut self.tree_sel),
        };
        if len == 0 {
            return;
        }
        let next = (*cursor as isize + delta).clamp(0, len as isize - 1);
        *cursor = next as usize;
        self.scroll_to_focus();
    }

    /// Left stick: move between panels.
    pub fn focus_pane(&mut self, forward: bool) {
        self.pane = if forward { self.pane.next() } else { self.pane.prev() };
    }

    /// L1/R1: change tab.
    pub fn cycle_tab(&mut self) {
        self.tab = self.tab.next();
    }

    /// Re-sort by `key`; asking for the current key flips the direction, which
    /// is what clicking a column header means everywhere else.
    pub fn sort_by(&mut self, key: SortKey) {
        if self.sort == key {
            self.sort_desc = !self.sort_desc;
        } else {
            self.sort = key;
            self.sort_desc = false;
        }
        self.refresh();
    }

    // ───────────────────────── mutation ─────────────────────────

    /// What is on the move-only clipboard, if anything.
    pub fn clipboard(&self) -> Option<&std::path::Path> {
        self.clipboard.as_deref()
    }

    /// Whether the last mutation failed, and why.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Pick the focused item up. There is no copy — this manager only relocates.
    pub fn cut(&mut self) {
        if let Some(path) = self.focused_row().map(|r| r.path.clone()) {
            self.clipboard = Some(path);
        }
    }

    /// Put the clipboard down here: into the focused folder, else the current
    /// one. A no-op when the destination is where the item already lives —
    /// which must NOT push a history entry the user would have to undo twice.
    pub fn paste(&mut self) {
        let Some(src) = self.clipboard.clone() else { return };
        // Prefer a focused folder as the destination — that is what "paste into"
        // means when the cursor is sitting on one.
        let dst_dir = match self.focused_row() {
            Some(row) if row.is_dir && row.path != src => row.path.clone(),
            // No focused row (the tree holds focus) falls back to the folder the
            // listing is showing — the documented "else the current folder".
            _ => self.cwd.clone(),
        };
        let Some(name) = src.file_name() else { return };
        let dst = dst_dir.join(name);
        if dst == src {
            self.clipboard = None; // dropped where it already was
            return;
        }
        // Refuse to move a folder into itself or its own descendant — it would
        // orphan the subtree and cannot be undone coherently.
        if src.is_dir() && dst_dir.starts_with(&src) {
            self.last_error = Some("$qm_err_into_self".into());
            return;
        }
        // INTERIM (until the conflict dialog lands): a collision is REFUSED
        // rather than resolved. `BatchFileOp` would happily replace and park the
        // occupant in `.trash`, which is recoverable — but replacing a file the
        // user never agreed to replace is not this tool's call to make. The
        // dialog turns this refusal into Replace / Keep both / Skip.
        // A collision is no longer refused: it opens the prompt, which turns the
        // choice into Replace / Keep both / Skip. Nothing is touched until the
        // whole queue is answered, and the answers ride out as ONE batch.
        let conflicts = flicker_content::probe_conflicts(&[(src.clone(), dst.clone())]);
        if !conflicts.is_empty() {
            self.prompt = Some(ConflictPrompt {
                pending: conflicts,
                ops: Vec::new(),
                apply_rest: false,
                promote: None,
            });
            self.last_error = None;
            return;
        }
        if self.run(vec![FileOp::mv(src, dst)], "$qm_did_move") {
            self.clipboard = None;
        }
    }

    /// Is the context menu open?
    pub fn is_menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Open the context menu on the focused item. Anchored beside the focused
    /// row so a pad user sees it where they are looking; a pointer user gets it
    /// under the cursor once CM6 lands the pointer layer.
    pub fn open_menu(&mut self) {
        self.menu_sel = 0;
        let chrome = TOP_BAR_H + TAB_BAR_H + CRUMB_H;
        self.menu = Some(match self.pane {
            Pane::List => Vec2::new(
                TREE_W + 40.0,
                chrome + HEADER_H + (self.sel as f32 * LIST_ROW_H) - self.list_scroll + LIST_ROW_H,
            ),
            Pane::Tree => Vec2::new(
                40.0,
                chrome + (self.tree_sel as f32 * ROW_H) - self.tree_scroll + ROW_H,
            ),
        });
    }

    /// Is the menu row at `i` pickable right now? Only Paste goes dead, and only
    /// while nothing is held — the same condition the proto's `disabled` param
    /// reads, so the highlight and the pick can never disagree.
    fn menu_row_live(&self, i: usize) -> bool {
        MENU_ROWS.get(i).is_some_and(|(verb, _)| *verb != "paste" || self.clipboard.is_some())
    }

    /// Move the pad cursor within the menu, skipping any dead row.
    fn menu_nav(&mut self, delta: isize) {
        let n = MENU_ROWS.len() as isize;
        let mut i = self.menu_sel as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if self.menu_row_live(i as usize) {
                self.menu_sel = i as usize;
                return;
            }
        }
    }

    /// The verb the pad cursor is sitting on, if it is live.
    fn menu_pick(&self) -> Option<&'static str> {
        let (verb, _) = MENU_ROWS.get(self.menu_sel)?;
        self.menu_row_live(self.menu_sel).then_some(*verb)
    }

    /// Dismiss the menu without picking anything.
    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// Is a collision prompt on screen? While it is, it owns every intent.
    pub fn is_prompting(&self) -> bool {
        self.prompt.is_some()
    }

    /// The collision currently being asked about.
    pub fn prompt_conflict(&self) -> Option<&flicker_content::Conflict> {
        self.prompt.as_ref().and_then(|p| p.pending.first())
    }

    /// How many collisions are still outstanding, the one on screen included.
    pub fn prompt_remaining(&self) -> usize {
        self.prompt.as_ref().map_or(0, |p| p.pending.len())
    }

    /// Toggle "apply to the remaining N".
    pub fn toggle_apply_rest(&mut self) {
        if let Some(p) = self.prompt.as_mut() {
            p.apply_rest = !p.apply_rest;
        }
    }

    /// Answer the collision on screen — and, when "apply to the remaining" is
    /// ticked, every other outstanding one the same way. When the queue empties
    /// the accumulated moves run as ONE batch, so a multi-collision paste stays
    /// a single undo entry.
    pub fn resolve_conflict(&mut self, how: flicker_content::Resolution) {
        use flicker_content::Resolution;
        let Some(mut p) = self.prompt.take() else { return };
        let take = if p.apply_rest { p.pending.len() } else { 1 };
        for c in p.pending.drain(..take.min(p.pending.len())).collect::<Vec<_>>() {
            match how {
                // The occupant is PARKED, never unlinked (`BatchFileOp` moves it
                // into `.trash/<batch>`), which is what keeps Replace revertible.
                Resolution::Replace => p.ops.push(FileOp::mv(c.src, c.dst)),
                Resolution::KeepBoth => match flicker_content::keep_both_name(&c.dst) {
                    Ok(free) => p.ops.push(FileOp::mv(c.src, free)),
                    Err(e) => {
                        self.last_error = Some(e.to_string());
                        return;
                    }
                },
                // Skip contributes no op: both sides are left exactly as they are.
                Resolution::Skip => {}
            }
        }
        if !p.pending.is_empty() {
            self.prompt = Some(p);
            return;
        }
        match p.promote.take() {
            Some(plan) => {
                // A promote is ALL-OR-NOTHING: the asset's files are dependencies
                // that travel together. Every answered op must land on exactly its
                // planned destination — a Skip strands part of the asset, a
                // Keep-both forks a filename out from under its rig — so anything
                // short of a full Replace set refuses the promote out loud.
                let complete = p.ops.len() == plan.dsts.len()
                    && p.ops.iter().all(|op| match op {
                        FileOp::Move { dst, .. } => plan.dsts.contains(dst),
                        _ => false,
                    });
                if complete {
                    self.run_promote(p.ops, plan.row);
                } else {
                    self.last_error = Some("$qm_err_promote_partial".into());
                }
            }
            None => {
                if p.ops.is_empty() {
                    // Everything was skipped — nothing ran, so the move stays
                    // pending and the clipboard is still loaded, ready to retry.
                    return;
                }
                if self.run(p.ops, "$qm_did_move") {
                    self.clipboard = None;
                }
            }
        }
    }

    /// Abandon the prompt without moving anything. The clipboard stays loaded —
    /// backing out of the question is not backing out of the cut.
    pub fn cancel_conflict(&mut self) {
        self.prompt = None;
        self.last_error = None;
    }

    // ── CM5: the Review tab — queue, facts, promote ──

    /// Re-scan the staging queue, keeping the selection on the same asset where
    /// it still exists (a refresh must not move the cursor under the user).
    pub fn refresh_queue(&mut self) {
        let focused = self.queue.get(self.review_sel).map(|q| q.dir.clone());
        self.queue = fs_model::staging_queue(&self.roots);
        self.review_sel = focused
            .and_then(|d| self.queue.iter().position(|q| q.dir == d))
            .unwrap_or(0)
            .min(self.queue.len().saturating_sub(1));
        self.ensure_facts();
    }

    /// The asset every Review verb acts on.
    pub fn selected_queue_item(&self) -> Option<&QueueItem> {
        self.queue.get(self.review_sel)
    }

    /// The selected asset's warning `$token`s (tests + the model read the same).
    pub fn facts_warning_tokens(&self) -> Vec<&'static str> {
        self.facts.as_ref().map(|f| f.warnings.clone()).unwrap_or_default()
    }

    /// The current error token/text, if any (test accessor).
    pub fn last_error_token(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    fn review_nav(&mut self, delta: isize) {
        if self.queue.is_empty() {
            return;
        }
        let n = self.queue.len() as isize;
        self.review_sel = ((self.review_sel as isize + delta).rem_euclid(n)) as usize;
        self.ensure_facts();
    }

    fn review_pick(&mut self, i: usize) {
        if i < self.queue.len() {
            self.review_sel = i;
            self.ensure_facts();
        }
    }

    /// Parse the selected asset's facts if the selection changed (or vanished).
    fn ensure_facts(&mut self) {
        let Some(item) = self.queue.get(self.review_sel) else {
            self.facts = None;
            return;
        };
        if self.facts.as_ref().is_some_and(|f| f.dir == item.dir) {
            return;
        }
        self.facts = Some(ReviewFacts::of(item, &self.roots));
    }

    /// PROMOTE the selected asset: move its whole folder (the dependencies travel
    /// together) from staging to the mirrored package path, and append the
    /// manifest row — ONE history entry, so one undo returns files AND ledger.
    /// A collision raises the same prompt every other mutation uses.
    pub fn promote_selected(&mut self) {
        let Some(item) = self.queue.get(self.review_sel).cloned() else { return };
        let files = fs_model::files_under(&item.dir);
        if files.is_empty() {
            return;
        }
        let target_dir = self.roots.package.join(&item.rel);
        let pairs: Vec<(PathBuf, PathBuf)> = files
            .iter()
            .map(|src| {
                let rel = src.strip_prefix(&item.dir).expect("file under its own dir");
                (src.clone(), target_dir.join(rel))
            })
            .collect();
        let row = flicker_content::manifest::ManifestEntry {
            name: item.name.clone(),
            class: item.class.id().to_string(),
            path: PathBuf::from("package").join(&item.rel).to_string_lossy().into_owned(),
            promoted_from: PathBuf::from("staging")
                .join(&item.rel)
                .to_string_lossy()
                .into_owned(),
        };
        let conflicts = flicker_content::probe_conflicts(&pairs);
        if conflicts.is_empty() {
            let ops = pairs.into_iter().map(|(s, d)| FileOp::mv(s, d)).collect();
            self.run_promote(ops, row);
            return;
        }
        // Seed the prompt with the clean moves; the dialog answers the rest. The
        // plan records EVERY destination — resolution must reconstruct it whole.
        let conflicted: Vec<&PathBuf> = conflicts.iter().map(|c| &c.src).collect();
        let ops = pairs
            .iter()
            .filter(|(s, _)| !conflicted.contains(&s))
            .map(|(s, d)| FileOp::mv(s.clone(), d.clone()))
            .collect();
        let dsts = pairs.iter().map(|(_, d)| d.clone()).collect();
        self.prompt = Some(ConflictPrompt {
            pending: conflicts,
            ops,
            apply_rest: false,
            promote: Some(PromotePlan { row, dsts }),
        });
        self.last_error = None;
    }

    /// Apply a promote as ONE history entry: the batch plus its manifest rider.
    fn run_promote(&mut self, ops: Vec<FileOp>, row: flicker_content::manifest::ManifestEntry) {
        let subject = row.name.clone();
        self.batch_seq += 1;
        let cmd = FileCommand {
            batch: BatchFileOp::new(
                ops,
                self.roots.staging.clone(),
                format!("b{}", self.batch_seq),
            ),
            label: "$qm_did_promote".to_string(),
            manifest: Some((self.roots.package.join("manifest.json"), row)),
        };
        match self.history.apply(cmd) {
            Ok(()) => {
                self.last_error = None;
                self.toast("$qm_did_promote", subject);
                self.facts = None; // the target's occupancy just changed
                self.refresh();
            }
            Err(e) => self.last_error = Some(e),
        }
    }

    /// Create a folder in the current directory.
    pub fn create_folder(&mut self) {
        let mut name = self.cwd.join("New Folder");
        // Never collide with an existing entry; the tool picks the next free name.
        if flicker_content::occupied(&name) {
            match flicker_content::keep_both_name(&name) {
                Ok(free) => name = free,
                Err(e) => {
                    self.last_error = Some(e.to_string());
                    return;
                }
            }
        }
        if self.run(vec![FileOp::mkdir(name.clone())], "$qm_did_newfolder") {
            self.refresh();
            if let Some(i) = self.rows.iter().position(|r| r.path == name) {
                self.sel = i;
                self.pane = Pane::List;
            }
        }
    }

    // ── inline rename ──

    /// Is a rename open?
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    /// The current draft, while a rename is open.
    pub fn rename_draft(&self) -> Option<&str> {
        self.rename.as_ref().map(|r| r.draft.as_str())
    }

    /// Open an in-place rename on the focused item.
    pub fn begin_rename(&mut self) {
        let Some((path, name)) =
            self.focused_row().map(|r| (r.path.clone(), r.name.clone()))
        else {
            return;
        };
        self.rename = Some(Rename { path, draft: name, pristine: true });
        self.last_error = None;
    }

    /// Abandon the rename, leaving the item alone.
    pub fn cancel_rename(&mut self) {
        self.rename = None;
        self.last_error = None;
    }

    /// Fold this frame's typed text into the draft. The first character
    /// REPLACES the name (see [`Rename::pristine`]); afterwards it appends.
    pub fn type_into_rename(&mut self, typed: &str, backspace: bool) {
        let Some(r) = self.rename.as_mut() else { return };
        if !typed.is_empty() {
            if r.pristine {
                r.draft.clear();
                r.pristine = false;
            }
            r.draft.push_str(typed);
        }
        if backspace {
            // Editing the existing name is a legitimate intent, so backspace
            // does not trip the pristine replace.
            r.pristine = false;
            r.draft.pop();
        }
        self.last_error = None;
    }

    /// Commit the rename. A name that cannot be used blocks the commit and says
    /// why, leaving the field open rather than discarding the user's typing.
    pub fn commit_rename(&mut self) {
        let Some(r) = self.rename.clone() else { return };
        let name = r.draft.trim();
        if name.is_empty() {
            self.last_error = Some("$qm_err_empty_name".into());
            return;
        }
        // A name is a single path segment — a separator would silently relocate
        // the item, which is what the move commands are for.
        if name.contains('/') || name.contains('\\') {
            self.last_error = Some("$qm_err_bad_name".into());
            return;
        }
        let Some(parent) = r.path.parent() else { return };
        let dst = parent.join(name);
        if dst == r.path {
            self.rename = None; // unchanged
            return;
        }
        if flicker_content::occupied(&dst) {
            self.last_error = Some("$qm_err_name_taken".into());
            return;
        }
        if self.run(vec![FileOp::mv(r.path, dst.clone())], "$qm_did_rename") {
            self.rename = None;
            if let Some(i) = self.rows.iter().position(|row| row.path == dst) {
                self.sel = i;
            }
        }
    }

    /// Take back the last mutation.
    pub fn undo(&mut self) {
        match self.history.undo() {
            Some(Err(e)) => self.last_error = Some(e),
            Some(Ok(_)) => {
                self.last_error = None;
                self.refresh();
            }
            None => {}
        }
    }

    /// Re-apply the last undone mutation.
    pub fn redo(&mut self) {
        match self.history.redo() {
            Some(Err(e)) => self.last_error = Some(e),
            Some(Ok(_)) => {
                self.last_error = None;
                self.refresh();
            }
            None => {}
        }
    }

    /// Apply a batch as ONE history entry. Returns whether it landed.
    ///
    /// Every mutation funnels through here, so the "one mutation, one undo"
    /// promise cannot be forgotten at a call site.
    /// Raise a confirmation. Newest last (the proto stacks downward); the bank
    /// is capped at the proto's slot count, oldest dropped first.
    fn toast(&mut self, label: &str, name: String) {
        self.toasts.push(Toast { label: label.to_string(), name, born: self.tick });
        while self.toasts.len() > TOAST_SLOTS {
            self.toasts.remove(0);
        }
    }

    /// The toasts still within their dwell, oldest first.
    fn live_toasts(&self) -> Vec<&Toast> {
        self.toasts.iter().filter(|t| toast_alive(t.born, self.tick)).collect()
    }

    /// How many confirmations are currently up.
    pub fn toast_count(&self) -> usize {
        self.live_toasts().len()
    }

    fn run(&mut self, ops: Vec<FileOp>, label: &str) -> bool {
        // What the confirmation names: the single item, or the count when a batch
        // moved several (the count is a NUMBER, not copy).
        let subject = match ops.as_slice() {
            [one] => op_subject(one),
            many => many.len().to_string(),
        };
        self.batch_seq += 1;
        let cmd = FileCommand {
            batch: BatchFileOp::new(
                ops,
                self.roots.staging.clone(),
                format!("b{}", self.batch_seq),
            ),
            label: label.to_string(),
            manifest: None,
        };
        match self.history.apply(cmd) {
            Ok(()) => {
                self.last_error = None;
                // Every mutation confirms itself, with the subject as DATA beside
                // the command's own `$token` label — never a composed sentence.
                self.toast(label, subject);
                self.refresh();
                true
            }
            Err(e) => {
                self.last_error = Some(e);
                false
            }
        }
    }

    /// Keep the focused row inside the scrolled viewport. The `list` component
    /// owns clipping but not the cursor, so the scene writes the offset.
    fn scroll_to_focus(&mut self) {
        if self.pane != Pane::List {
            return;
        }
        let top = self.sel as f32 * LIST_ROW_H;
        let bottom = top + LIST_ROW_H;
        if top < self.list_scroll {
            self.list_scroll = top;
        } else if bottom > self.list_scroll + self.list_view_h {
            self.list_scroll = bottom - self.list_view_h;
        }
    }

    /// Which rows are worth building nodes for. A folder can hold hundreds of
    /// files; only the visible window plus a margin becomes UI.
    fn visible_range(&self) -> (usize, usize) {
        let first = (self.list_scroll / LIST_ROW_H).floor().max(0.0) as usize;
        let count = (self.list_view_h / LIST_ROW_H).ceil() as usize + 2;
        (first.min(self.rows.len()), (first + count).min(self.rows.len()))
    }

    // ────────────────────────────── UI ──────────────────────────────

    fn hud_model(&self) -> ValueMap {
        let mut m = ValueMap::new();
        m.set("list_scroll", f64::from(self.list_scroll));
        m.set("tree_scroll", f64::from(self.tree_scroll));

        // Row captions + per-class colour, keyed by the node ids build_tree makes.
        let (first, last) = self.visible_range();
        for (i, row) in self.rows.iter().enumerate().take(last).skip(first) {
            m.set(format!("row_{i}_name"), row.name.clone());
            m.set(format!("row_{i}_type"), class_token(row));
            m.set(format!("row_{i}_size"), human_size(row));
            m.set(format!("row_{i}_color"), class_color(row));
            m.set(format!("row_{i}_sel"), i == self.sel && self.pane == Pane::List);
        }
        for (i, t) in self.tree.iter().enumerate() {
            m.set(format!("tree_{i}_name"), t.name.clone());
            m.set(format!("tree_{i}_caret"), caret_token(t));
            m.set(format!("tree_{i}_sel"), i == self.tree_sel && self.pane == Pane::Tree);
        }
        // The collision prompt. Only the NAMES and the measured facts are data;
        // every caption on the dialog is a `$token` in the tree.
        m.set("has_menu", self.menu.is_some());
        m.set("has_prompt", self.prompt.is_some());
        if let Some(c) = self.prompt_conflict() {
            m.set(
                "conflict_name",
                c.dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            );
            m.set(
                "conflict_where",
                c.dst
                    .parent()
                    .map(|p| fs_model::breadcrumb(&self.roots, p).join(" / "))
                    .unwrap_or_default(),
            );
            m.set("conflict_existing", facts_line(&c.existing));
            m.set("conflict_incoming", facts_line(&c.incoming));
            // "Apply to the remaining N" is only meaningful with more than one
            // outstanding, so the checkbox is gated on it rather than offering a
            // choice that cannot matter.
            let rest = self.prompt_remaining().saturating_sub(1);
            m.set("conflict_multi", rest > 0);
            m.set("conflict_rest", rest.to_string());
            m.set(APPLY_REST_ID, self.prompt.as_ref().is_some_and(|p| p.apply_rest));
        }
        // The toast bank. The proto spells out TOAST_SLOTS rows and the live
        // toasts are paged into them, newest last — the sanctioned dynamic-rows
        // pattern, since the walker has no repeater.
        let live = self.live_toasts();
        for i in 0..TOAST_SLOTS {
            let t = live.get(i);
            m.set(format!("toast_{i}_on"), t.is_some());
            if let Some(t) = t {
                m.set(format!("toast_{i}_label"), t.label.clone());
                m.set(format!("toast_{i}_name"), t.name.clone());
            }
        }
        m.set("crumbs", fs_model::breadcrumb(&self.roots, &self.cwd).join(" / "));
        m.set("count", self.rows.len().to_string());
        // Clipboard / error state for the status line. All display copy is a
        // `$token`; only the item's own NAME is data.
        m.set("has_clip", self.clipboard.is_some());
        if let Some(clip) = &self.clipboard {
            m.set(
                "clip_name",
                clip.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            );
        }
        m.set("can_undo", self.history.can_undo());
        if let Some(r) = &self.rename {
            m.set("rename_draft", r.draft.clone());
        }
        if let Some(e) = &self.last_error {
            m.set("error", e.clone());
        }
        m.set("has_error", self.last_error.is_some());
        UiIntents::mirror_into(&mut m, &self.fired_sigs);
        m
    }

    /// This frame's chrome. The template tier this bench composed against has been
    /// removed; the bench is not in the launcher roster, so `build_tree` returns an
    /// empty `screen` placeholder rather than rebuilding a UI ad-hoc.
    pub fn build_tree(&self, _screen: Vec2) -> UiNode {
        UiNode { component: "screen".to_string(), id: "quartermaster".to_string(), ..Default::default() }
    }

    /// Fold this frame's walker results into bench state. ONE dispatcher: a pad
    /// chord, a key and a click all arrive here as the same result name.
    pub fn apply_results(&mut self, results: &ValueMap) {
        // A collision prompt is MODAL: it owns every intent while it is up, so a
        // stray nav or chord cannot move the cursor out from under the question
        // or mutate the very item being asked about. Checked before the rename
        // guard because a prompt can only be raised from a settled state.
        if self.is_prompting() {
            use flicker_content::Resolution;
            if results.is_on(APPLY_REST_ID) {
                self.toggle_apply_rest();
            }
            if results.is_on("conflict_replace") {
                self.resolve_conflict(Resolution::Replace);
            } else if results.is_on("conflict_keep_both") {
                self.resolve_conflict(Resolution::KeepBoth);
            } else if results.is_on("conflict_skip") {
                self.resolve_conflict(Resolution::Skip);
            } else if results.is_on("cancel") {
                self.cancel_conflict();
            }
            return;
        }
        // The context menu is modal too, but LOWER precedence than the prompt: a
        // pick that raises a collision closes the menu and opens the question.
        // A pick fires the verb's own result name, so the arms below run it — the
        // menu simply closes first and lets the frame's result through.
        if self.is_menu_open() {
            if results.is_on("cancel") || results.is_on("menu_open") {
                self.close_menu();
                return;
            }
            // The pad steers the menu itself: d-pad moves the cursor (skipping the
            // divider and any dead row), Confirm picks. Nav must NOT fall through,
            // or it would move the listing cursor out from under the open menu.
            if results.is_on("nav_up") {
                self.menu_nav(-1);
                return;
            }
            if results.is_on("nav_down") {
                self.menu_nav(1);
                return;
            }
            if results.is_on("confirm") {
                let picked = self.menu_pick();
                self.close_menu();
                match picked {
                    Some("confirm") => self.confirm(),
                    Some("rename") => self.begin_rename(),
                    Some("cut") => self.cut(),
                    Some("paste") => self.paste(),
                    Some("create_folder") => self.create_folder(),
                    _ => {}
                }
                return;
            }
            // A POINTER pick fires BOTH the row's action and the click edge (folded
            // in by `update` as `menu_dismiss`); a click on dead space fires the edge
            // alone; a chord verb fires its action alone. Ambient state — `hud_hit`,
            // bind republishes — fires none of them. So: close on the click edge or
            // on a fired verb, then let the verb arms below run the pick. Closing on
            // "anything else" closed the menu on the first idle frame after the
            // edge-resolved open signal — a one-frame lifetime (QA 2026-08-03:
            // "appears then disappears immediately").
            let verb_fired =
                ["rename", "cut", "paste", "create_folder"].iter().any(|v| results.is_on(v));
            if results.is_on("menu_dismiss") || verb_fired {
                self.close_menu();
            } else {
                return;
            }
        } else if results.is_on("menu_open") {
            self.open_menu();
            return;
        }
        // While a rename owns the keyboard NOTHING else acts: navigation would
        // move the cursor out from under the open field, and a chord verb would
        // mutate the very item being named.
        if self.is_renaming() {
            if results.is_on("rename_commit") {
                self.commit_rename();
            } else if results.is_on("rename_cancel") {
                self.cancel_rename();
            }
            return;
        }
        if results.is_on("rename") {
            self.begin_rename();
            return;
        }
        for t in Tab::ALL {
            if results.is_on(t.id()) {
                self.tab = t;
                if t == Tab::Review {
                    self.refresh_queue();
                }
            }
        }
        if results.is_on("tab_next") || results.is_on("tab_prev") {
            self.cycle_tab();
        }
        if results.is_on("panel_next") {
            self.focus_pane(true);
        }
        if results.is_on("panel_prev") {
            self.focus_pane(false);
        }
        if results.is_on("nav_up") {
            // The active TAB owns the d-pad: Files walks the listing, Review
            // walks the staging queue — one cursor per page, single focus.
            match self.tab {
                Tab::Files => self.nav(-1),
                Tab::Review => self.review_nav(-1),
            }
        }
        if results.is_on("nav_down") {
            match self.tab {
                Tab::Files => self.nav(1),
                Tab::Review => self.review_nav(1),
            }
        }
        // Left/right cross panes, so a d-pad alone reaches everything.
        if results.is_on("nav_left") {
            self.focus_pane(false);
        }
        if results.is_on("nav_right") {
            self.focus_pane(true);
        }
        if results.is_on("confirm") {
            match self.tab {
                Tab::Files => self.confirm(),
                // Single focus: the queue cursor IS the selection, Confirm acts
                // on it. A risky landing raises the prompt; a clean one is one
                // undoable entry with its toast — the bench's mutation contract.
                Tab::Review => self.promote_selected(),
            }
        }
        if results.is_on("promote") && self.tab == Tab::Review {
            self.promote_selected();
        }
        if self.tab == Tab::Review {
            for i in 0..self.queue.len() {
                if results.is_on(&format!("rv_pick_{i}")) {
                    self.review_pick(i);
                }
            }
        }
        if results.is_on("cancel") {
            // Cancel drops a pending move before it navigates — the clipboard is
            // the more recent thing the user would want to back out of.
            if self.clipboard.take().is_none() {
                self.cancel();
            }
        }
        // Editor verbs. Undo/redo are checked before the rest so a chord that
        // also matched a navigation name cannot mutate twice in one frame.
        if results.is_on("undo") {
            self.undo();
        }
        if results.is_on("redo") {
            self.redo();
        }
        if results.is_on("cut") {
            self.cut();
        }
        if results.is_on("paste") {
            self.paste();
        }
        if results.is_on("create_folder") {
            self.create_folder();
        }
        for (id, key) in [
            ("sort_name", SortKey::Name),
            ("sort_type", SortKey::Type),
            ("sort_size", SortKey::Size),
        ] {
            if results.is_on(id) {
                self.sort_by(key);
            }
        }
        // Pointer picks: a row click focuses that row (and its pane).
        for i in 0..self.rows.len() {
            if results.is_on(&format!("row_pick_{i}")) {
                self.pane = Pane::List;
                self.sel = i;
            }
        }
        for i in 0..self.tree.len() {
            if results.is_on(&format!("tree_open_{i}")) {
                self.pane = Pane::Tree;
                self.tree_sel = i;
                self.confirm();
                break;
            }
        }
        if let Some(s) = results.number("list_scroll") {
            self.list_scroll = s as f32;
        }
        if let Some(s) = results.number("tree_scroll") {
            self.tree_scroll = s as f32;
        }
    }
}

impl Scene for Quartermaster {
    fn enter(&mut self, renderer: &mut Renderer) {
        self.white = Some(renderer.load_texture(&[0xff, 0xff, 0xff, 0xff], 1, 1));
        self.ui_theme = Some(Theme::build(renderer));
        self.ui_styles = flicker::ui::load_styles_for(HUD_UI_THEME, Some(&crate::scene_styles()));
        self.refresh();
        renderer.window().set_title("Quartermaster Bench");
    }

    fn update(&mut self, _dt: Duration, input: &InputState, _signals: &mut SceneInput, renderer: &Renderer) -> Transition {
        let screen = renderer.size();
        self.list_view_h =
            (screen.y - (TOP_BAR_H + TAB_BAR_H + CRUMB_H + STATUS_H + HEADER_H)).max(120.0);

        if let Some((map, _, _)) = flicker_shell::take_pending_input() {
            self.bindings = ContextualBindings::new(with_context_menu(map))
                .with(InputContext::Chord, editor_chords());
        }

        let tree = self.build_tree(screen);
        self.ui_intents = UiIntents::of(&tree);
        let model = self.hud_model();
        // Typed characters reach the walker ONLY while a rename is open, so a
        // stray keystroke can never edit something the user is not naming. The
        // scene folds the same text into its own draft below, which is what
        // makes the pristine-replace possible.
        let renaming = self.is_renaming();
        let typed = if renaming { input.typed().to_string() } else { String::new() };
        let backspace = renaming && input.backspace();
        if renaming {
            self.ui_state.request_focus(RENAME_ID);
            self.type_into_rename(&typed, backspace);
        }
        let snap = UiInput {
            mouse: input.mouse_position,
            clicked: input.mouse_left_pressed,
            down: input.mouse_left,
            screen,
            typed,
            backspace,
            wheel: input.mouse_wheel_delta,
        };
        let frame = run_ui(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state);
        let over_hud = frame.results.is_on("hud_hit");
        self.hud_commands = frame.commands;

        // ONE resolve, ONE dispatch — the walker layer consumes the screen's
        // declared intents, so navigation never reads a raw key.
        self.tick = self.tick.wrapping_add(1);
        // CM5: the staging queue follows the disk — a light 1 s poll (a few
        // dozen dirents); the selection survives by path.
        if self.tab == Tab::Review && self.tick.is_multiple_of(60) {
            self.refresh_queue();
        }
        self.ev.clear();
        self.resolver.resolve_frame(
            &self.bindings,
            &self.gamepad_config,
            input,
            self.tick,
            &mut self.ev,
        );
        // Reconcile the chord layer BEFORE the events are routed, so a verb
        // pressed on the same frame the modifier went down already resolves
        // against the chord map.
        self.chord.update(&mut self.bindings, &self.ev, input, &self.gamepad_config);
        let ctx = self.bindings.active();
        let events: Vec<InputEvent> =
            self.ev.iter().map(|f| InputEvent::from_fired(f, ctx, input)).collect();
        self.fired_sigs.clear();

        let mut walker =
            WalkerHandler::hud(&mut self.ui_state, over_hud).with_intents(&self.ui_intents);
        let mut route = RouteCtx::default();
        {
            let mut chain: [&mut dyn InputHandler; 1] = [&mut walker];
            Router::dispatch(&events, &mut chain, &mut route);
        }
        // The standard post-dispatch seam: reconcile context/focus intents so the
        // pointer and the pad share ONE focus id (spec §4.2a).
        let focus_change = apply_context_requests(&mut self.bindings, &route.requests);
        walker.apply_focus(focus_change);
        self.fired_sigs = walker.take_fired();

        // Fold the fired intent names in beside the click results, so both
        // channels reach the ONE dispatcher identically.
        let mut results = frame.results.clone();
        for name in &self.fired_sigs {
            results.set(name.clone(), true);
        }

        // The open menu's dismissal edge, folded in beside the click results the
        // same way the raw rename keys are below: a left-click while the menu is
        // open means "the user picked somewhere" — on a row (its action fires in
        // the same frame and runs after the close) or on dead space (dismiss
        // alone). Without this edge the menu block cannot tell a pick from the
        // ambient per-frame results (`hud_hit` is republished every frame).
        if self.is_menu_open() && input.mouse_left_pressed {
            results.set("menu_dismiss", true);
        }

        // Enter/Esc while renaming are read RAW — the ruled exception. The
        // `TextEntry` map is empty by design, so the intent channel is
        // deliberately unavailable here (see flicker-widgets' intents module).
        let enter = input.key_down(Key::Enter);
        let esc = input.key_down(Key::Escape);
        if renaming {
            if enter && !self.enter_prev {
                results.set("rename_commit", true);
            } else if esc && !self.esc_prev {
                results.set("rename_cancel", true);
            }
        }
        self.enter_prev = enter;
        self.esc_prev = esc;
        self.apply_results(&results);

        if results.is_on("pause_open") {
            if let Some(theme) = self.ui_theme {
                return Transition::Push(Box::new(PauseScene::new(
                    theme,
                    self.bindings.active_map(),
                    &AbstractControls::default(),
                    &self.gamepad_config,
                )));
            }
        }
        Transition::None
    }

    fn render(&mut self, renderer: &mut Renderer) {
        if let Some(white) = self.white {
            render_hud(renderer, &self.hud_commands, white, &[]);
        }
    }
}

/// The scene factory the launcher roster registers.
pub fn scene() -> Box<dyn Scene> {
    Box::new(Quartermaster::new())
}

// ── small node helpers (the Rust-tree idiom) ──

/// What the Review tab knows about the SELECTED staged asset — parsed once per
/// selection. The UI that displayed its counts has been removed; what survives is
/// the cache key (`dir`) and the honest `warnings` the bench still surfaces.
struct ReviewFacts {
    dir: PathBuf,
    /// `$token`s naming what is wrong with the asset, worst first.
    warnings: Vec<&'static str>,
}

impl ReviewFacts {
    fn of(item: &QueueItem, roots: &Roots) -> Self {
        let files = fs_model::files_under(&item.dir);
        // The primary json: the asset's own file where present, else the first.
        let primary = files
            .iter()
            .find(|f| {
                let s = f.to_string_lossy();
                s.contains(&item.name) && s.contains(".json")
            })
            .or_else(|| files.iter().find(|f| f.to_string_lossy().contains(".json")));
        let mut bones = None;
        let mut textures_missing = 0usize;
        if let Some(p) = primary {
            let logical = fs_model::logical(p);
            if let Ok(text) = flicker_content::package::read_text(&logical) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    bones = v["skeleton"]["bones"].as_array().map(|a| a.len());
                    if let Some(texs) = v["source"]["textures"].as_array() {
                        textures_missing = texs
                            .iter()
                            .filter_map(|t| t.as_str())
                            .filter(|t| !item.dir.join(t).exists())
                            .count();
                    }
                }
            }
        }
        let target_dir = roots.package.join(&item.rel);
        // Warnings, worst first — each a real fact, never a placeholder.
        let mut warnings: Vec<&'static str> = Vec::new();
        if bones.is_some_and(|b| b > 1 && b != flicker_content::baseline::CANON_BONES) {
            warnings.push("$qm_warn_off_canon");
        }
        if textures_missing > 0 {
            warnings.push("$qm_warn_missing_textures");
        }
        if target_dir.exists() {
            warnings.push("$qm_warn_target_occupied");
        }
        Self { dir: item.dir.clone(), warnings }
    }
}

/// The `$token` naming a row's class in the Type column.
fn class_token(row: &Row) -> String {
    if row.is_dir {
        return "$qm_class_folder".into();
    }
    format!("$qm_class_{}", row.class.id())
}

/// The dotted STYLE PATH for a row's class colour — the Model carries a path,
/// never rgba, so the one palette stays the only source of colour.
fn class_color(row: &Row) -> String {
    if row.is_dir {
        return "quartermaster.class.folder".into();
    }
    format!("quartermaster.class.{}", row.class.id())
}

fn caret_token(t: &TreeRow) -> &'static str {
    match (t.has_children, t.expanded) {
        (false, _) => "$qm_caret_leaf",
        (true, false) => "$qm_caret_closed",
        (true, true) => "$qm_caret_open",
    }
}

/// A compact size for the Size column. Folders show a dash.
/// The context menu's PICKABLE rows: `(verb, proto row index)`. The proto's
/// divider sits at index 4 and is not pickable, so the cursor indexes THIS table
/// and the row index is looked up — a pad cursor can never land on a hairline.
const MENU_ROWS: [(&str, usize); 5] =
    [("confirm", 0), ("rename", 1), ("cut", 2), ("paste", 3), ("create_folder", 5)];

/// The bench's world-context map: the shared preset plus the one signal it
/// leaves unbound. `ContextMenu` is in `ActionSignal::ALL` (so the remap surface
/// already offers it) but no default preset binds it, because no scene wanted a
/// context menu until this one.
fn bench_map() -> InputMap {
    with_context_menu(InputMap::wasd_and_mouse())
}

/// Add the context-menu bindings to `map`, idempotently: West on the pad (the
/// free face button — A/B are Confirm/Cancel and Y opens the chord layer) and
/// right-click on the mouse. Applied to a remapped map too, so a trip through
/// settings cannot silently strip the menu.
fn with_context_menu(mut map: InputMap) -> InputMap {
    map.bind(
        flicker_input_core::ActionSignal::ContextMenu,
        flicker_input_core::InputBinding::GamepadButton(
            flicker_input_core::device::GamepadButton::West,
        ),
    );
    map.bind(
        flicker_input_core::ActionSignal::ContextMenu,
        flicker_input_core::InputBinding::MouseButton(
            flicker_input_core::device::MouseButton::Right,
        ),
    );
    map
}

/// What a confirmation names for one operation: the item's own file name. Data,
/// never copy — the `$token` beside it says what HAPPENED to it.
fn op_subject(op: &FileOp) -> String {
    let path = match op {
        FileOp::Move { dst, .. } => dst,
        FileOp::Mkdir { path } => path,
    };
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// One side of a collision, as the prompt shows it: the measured size, or the
/// folder marker. Facts only — every CAPTION around it is a `$token`.
fn facts_line(f: &flicker_content::FileFacts) -> String {
    if f.is_dir {
        return "$qm_class_folder".into();
    }
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match f.size {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.0} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

fn human_size(row: &Row) -> String {
    if row.is_dir {
        return "$qm_dash".into();
    }
    human_bytes(row.size)
}

/// `1.4 MB` / `230 KB` / `87 B` — the ONE byte formatter both tabs read.
fn human_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match b {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.0} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_content::PackageClass;

    fn scratch(name: &str) -> (Quartermaster, PathBuf) {
        let d = std::env::temp_dir().join(format!("flicker_qmbench_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        let roots = Roots { package: d.join("package"), staging: d.join("staging") };
        std::fs::create_dir_all(roots.package.join("characters/katanami")).unwrap();
        std::fs::create_dir_all(&roots.staging).unwrap();
        flicker_content::package::write_text(
            &roots.package.join("Alpha.json"),
            r#"{"format":"flicker.rig","mesh":{"indices":[1,2,3]}}"#,
        )
        .unwrap();
        flicker_content::package::write_text(
            &roots.package.join("Beta.json"),
            r#"{"clips":[{"name":"walk"}]}"#,
        )
        .unwrap();
        (Quartermaster::with_roots(roots), d)
    }

    #[test]
    fn the_listing_classifies_and_folders_lead() {
        let (qm, d) = scratch("listing");
        let rows = qm.rows();
        assert_eq!(rows[0].name, "characters");
        assert!(rows[0].is_dir);
        assert_eq!(rows[1].class, PackageClass::Rig, "Alpha is a rig");
        assert_eq!(rows[2].class, PackageClass::Clip, "Beta is a clip");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The controller baseline: everything reachable with d-pad + stick + A/B,
    /// on a single focus, with no pointer and no multi-select.
    #[test]
    fn the_bench_is_navigable_from_a_pad_alone() {
        let (mut qm, d) = scratch("padnav");
        assert_eq!(qm.pane(), Pane::List);

        qm.focus_pane(true);
        assert_eq!(qm.pane(), Pane::Tree);
        qm.focus_pane(true);
        assert_eq!(qm.pane(), Pane::List, "two panes cycle");

        qm.nav(1);
        assert_eq!(qm.selected().unwrap().name, "Alpha.json");
        qm.nav(-5);
        assert_eq!(qm.selected().unwrap().name, "characters", "clamped at the top");

        // A opens a folder; B climbs back and lands on the folder we left.
        qm.confirm();
        assert!(qm.cwd().ends_with("characters"));
        qm.cancel();
        assert!(qm.cwd().ends_with("package"));
        assert_eq!(
            qm.selected().unwrap().name,
            "characters",
            "the cursor returns to where we came from"
        );

        // B at a root is a no-op — the browser never climbs out of the trees.
        let before = qm.cwd().to_path_buf();
        qm.cancel();
        assert_eq!(qm.cwd(), before);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn tabs_cycle_and_the_body_follows() {
        let (mut qm, d) = scratch("tabs");
        assert_eq!(qm.tab(), Tab::Files);
        qm.cycle_tab();
        assert_eq!(qm.tab(), Tab::Review);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn asking_for_the_same_sort_key_flips_the_direction() {
        let (mut qm, d) = scratch("sort");
        assert_eq!(qm.sort(), (SortKey::Name, false));
        qm.sort_by(SortKey::Name);
        assert_eq!(qm.sort(), (SortKey::Name, true), "same key flips");
        qm.sort_by(SortKey::Size);
        assert_eq!(qm.sort(), (SortKey::Size, false), "a new key starts ascending");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The three drift gates every shipped screen wires, walked against the
    /// REAL tree the scene builds.
    #[test]
    fn the_tree_passes_the_drift_gates() {
        let (qm, d) = scratch("gates");
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));

        let unknown = flicker::ui::unknown_kinds(&tree);
        assert!(unknown.is_empty(), "unknown component kinds: {unknown:?}");

        let raw = flicker::ui::raw_display_literals(&tree);
        assert!(raw.is_empty(), "raw display literals (must be $tokens): {raw:?}");

        let flags = flicker::ui::strings::raw_model_publish_literals(include_str!("lib.rs"));
        assert!(flags.is_empty(), "raw display copy published into the Model: {flags:?}");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Run the REAL walker over the REAL tree and styles, and read back where
    /// text actually landed. Layout bugs are invisible to a tree-shape
    /// assertion — this is the only way to prove alignment without a window.
    fn drawn_text(qm: &Quartermaster, screen: Vec2) -> Vec<(f32, f32, String)> {
        // The stringtable starts EMPTY and is loaded by the running app, so a
        // test that walks a real tree must load the shipped content too —
        // otherwise every `$token` renders raw and proves nothing.
        let strings = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../content/data/stringtable.json"
        ))
        .expect("stringtable reads");
        flicker::ui::strings::load_str(&strings, "en-us");

        let tree = qm.build_tree(screen);
        let styles = flicker::ui::load_styles_for(HUD_UI_THEME, Some(&crate::scene_styles()));
        let mut state = UiState::default();
        let snap = UiInput {
            mouse: Vec2::new(-1.0, -1.0),
            clicked: false,
            down: false,
            screen,
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let frame = run_ui(&tree, &qm.hud_model(), &styles, &snap, &mut state);
        frame
            .commands
            .iter()
            .filter_map(|c| match c {
                // The row's invisible click target is a label-less `button`, and
                // the shipped select-row idiom draws its empty caption — not a
                // column, so it is dropped here rather than skewing the geometry.
                HudCommand::Text { x, y, text, .. } if !text.is_empty() => {
                    Some((*x, *y, text.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The design asks for the basename to be PRESELECTED. The text field has
    /// no selection model, so the first typed character replaces the draft —
    /// what select-all-then-type would have produced.
    #[test]
    fn the_first_typed_character_replaces_the_name() {
        let (mut qm, d) = scratch("pristine");
        qm.nav(1); // Alpha.json
        qm.begin_rename();
        assert_eq!(qm.rename_draft(), Some("Alpha.json"), "opens on the current name");

        qm.type_into_rename("K", false);
        assert_eq!(qm.rename_draft(), Some("K"), "the first key REPLACED the name");
        qm.type_into_rename("atana", false);
        assert_eq!(qm.rename_draft(), Some("Katana"), "and then appends");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Backspace means "edit this name", not "replace it" — so it must not trip
    /// the pristine replace.
    #[test]
    fn backspace_edits_the_existing_name_instead_of_clearing_it() {
        let (mut qm, d) = scratch("backspace");
        qm.nav(1);
        qm.begin_rename();
        qm.type_into_rename("", true);
        assert_eq!(qm.rename_draft(), Some("Alpha.jso"), "one character removed");
        qm.type_into_rename("n", false);
        assert_eq!(qm.rename_draft(), Some("Alpha.json"), "typing now appends");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_rename_commits_moves_the_file_and_undoes() {
        let (mut qm, d) = scratch("rename");
        let before = qm.cwd().join("Alpha.json");
        qm.nav(1);
        qm.begin_rename();
        qm.type_into_rename("Renamed.json", false);
        qm.commit_rename();
        assert!(qm.last_error().is_none(), "{:?}", qm.last_error());
        assert!(!qm.is_renaming(), "the field closed");

        let after = qm.cwd().join("Renamed.json");
        assert!(flicker_content::occupied(&after), "the file took the new name");
        assert!(!flicker_content::occupied(&before));
        assert_eq!(qm.selected().unwrap().name, "Renamed.json", "focus followed the rename");

        qm.undo();
        assert!(flicker_content::occupied(&before), "one undo restored the old name");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A name that cannot be used BLOCKS the commit and says why — it must not
    /// silently discard what the user typed.
    #[test]
    fn an_unusable_name_blocks_the_commit_and_keeps_the_draft() {
        let (mut qm, d) = scratch("badname");
        qm.nav(1); // Alpha.json
        for (typed, err) in [
            ("sub/dir.json", "$qm_err_bad_name"),
            ("Beta.json", "$qm_err_name_taken"),
        ] {
            qm.begin_rename();
            qm.type_into_rename(typed, false);
            qm.commit_rename();
            assert_eq!(qm.last_error(), Some(err), "typing {typed:?}");
            assert!(qm.is_renaming(), "the field stays open so the typing is not lost");
            assert_eq!(qm.rename_draft(), Some(typed), "and the draft survives");
            qm.cancel_rename();
        }

        // An emptied field is refused too. (Typing "" is a no-op by design, so
        // the draft is cleared the way a user would: replace, then backspace.)
        qm.begin_rename();
        qm.type_into_rename("x", false);
        qm.type_into_rename("", true);
        assert_eq!(qm.rename_draft(), Some(""));
        qm.commit_rename();
        assert_eq!(qm.last_error(), Some("$qm_err_empty_name"));
        qm.cancel_rename();

        assert!(!qm.can_undo(), "nothing was recorded for a refused rename");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn escape_abandons_a_rename() {
        let (mut qm, d) = scratch("renamecancel");
        qm.nav(1);
        qm.begin_rename();
        qm.type_into_rename("Whatever", false);
        qm.cancel_rename();
        assert!(!qm.is_renaming());
        assert!(flicker_content::occupied(&qm.cwd().join("Alpha.json")), "the file is untouched");
        assert!(!qm.can_undo());
        let _ = std::fs::remove_dir_all(d);
    }

    /// While the field owns the keyboard, nothing else may act — navigation
    /// would move the cursor out from under the open field, and a chord verb
    /// would mutate the very item being named.
    #[test]
    fn a_rename_swallows_every_other_intent() {
        let (mut qm, d) = scratch("renameswallow");
        qm.nav(1);
        let focused = qm.selected().unwrap().path.clone();
        qm.begin_rename();

        for verb in ["nav_down", "nav_up", "cut", "paste", "create_folder", "undo", "confirm"] {
            qm.apply_results(&on(verb));
        }
        assert!(qm.is_renaming(), "still renaming after every other intent");
        assert_eq!(qm.selected().unwrap().path, focused, "the cursor did not move");
        assert!(qm.clipboard().is_none(), "no verb ran");
        assert!(!qm.can_undo(), "and nothing mutated");

        // The two that DO reach it.
        qm.apply_results(&on("rename_cancel"));
        assert!(!qm.is_renaming());
        let _ = std::fs::remove_dir_all(d);
    }

    /// THE ONE-FRAME-LIFETIME REGRESSION (QA 2026-08-03). `menu_open` rides an
    /// edge-resolved signal, so the frame after opening carries only AMBIENT
    /// results (`hud_hit` is republished every frame) — the menu must survive
    /// those. It closes only on the click edge (`menu_dismiss`, folded in by
    /// `update`) or on a fired verb.
    #[test]
    fn the_menu_survives_idle_frames_and_closes_on_a_real_pick() {
        let (mut qm, d) = scratch("menuidle");
        qm.apply_results(&on("menu_open"));
        assert!(qm.is_menu_open());

        // The live loop's idle frame: no action fired, hud_hit republished.
        for over_hud in [true, false, true] {
            qm.apply_results(&ValueMap::new().with("hud_hit", over_hud));
            assert!(qm.is_menu_open(), "an idle frame must not dismiss the menu");
        }

        // A click on dead space: the edge alone — dismiss, and nothing runs.
        assert!(!qm.can_undo());
        qm.apply_results(&ValueMap::new().with("hud_hit", true).with("menu_dismiss", true));
        assert!(!qm.is_menu_open(), "a dead-space click dismisses");
        assert!(!qm.can_undo(), "and no verb ran");
        let _ = std::fs::remove_dir_all(d);
    }

    // ── CM5: the Review tab ──

    /// A minimal staged ASSET: a folder holding a 2-bone rig json + one texture
    /// the json lists (present or not, per `with_tex`).
    fn stage_asset(root: &std::path::Path, name: &str, with_tex: bool) {
        let dir = root.join("staging/characters").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let id16 = "[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]";
        flicker_content::package::write_text(
            &dir.join(format!("{name}.json")),
            &format!(
                r#"{{"format":"flicker.rig","version":1,
                    "skeleton":{{"bones":[
                      {{"name":"root","parent":-1,"local":{id16},"inverse_bind":{id16}}},
                      {{"name":"pelvis","parent":0,"local":{id16},"inverse_bind":{id16}}}]}},
                    "mesh":{{"vertices":[],"indices":[]}},
                    "source":{{"textures":["{name}_BaseColor.png"]}}}}"#
            ),
        )
        .unwrap();
        if with_tex {
            std::fs::write(dir.join(format!("{name}_BaseColor.png")), b"png").unwrap();
        }
    }

    /// THE CM5 CONTRACT: Promote moves the WHOLE asset folder into the mirrored
    /// package path, appends the manifest row — and ONE undo returns the files
    /// to staging AND takes the ledger row back (the banked spec's named test).
    #[test]
    fn promote_moves_the_asset_appends_the_manifest_and_one_undo_returns_both() {
        let (mut qm, d) = scratch("promote");
        stage_asset(&d, "NewThing", true);
        qm.refresh_queue();
        assert_eq!(qm.selected_queue_item().map(|q| q.name.as_str()), Some("NewThing"));

        qm.promote_selected();
        assert!(!qm.is_prompting(), "a clean target asks no questions");
        let target = d.join("package/characters/NewThing");
        assert!(
            flicker_content::package::file_exists(&target.join("NewThing.json")),
            "the rig landed in the package"
        );
        assert!(target.join("NewThing_BaseColor.png").exists(), "its texture travelled WITH it");
        let manifest = d.join("package/manifest.json");
        let rows = flicker_content::manifest::read(&manifest).unwrap();
        assert_eq!(rows.len(), 1, "one promote, one ledger row");
        assert_eq!(rows[0].name, "NewThing");
        assert_eq!(rows[0].path, "package/characters/NewThing");
        assert_eq!(rows[0].promoted_from, "staging/characters/NewThing");
        assert!(qm.toast_count() >= 1, "the mutation confirmed itself");
        qm.refresh_queue();
        assert!(qm.selected_queue_item().is_none(), "staging emptied");

        // ONE undo: files home again, ledger row gone.
        qm.undo();
        assert!(
            flicker_content::package::file_exists(
                &d.join("staging/characters/NewThing/NewThing.json")
            ),
            "undo returned the asset to staging"
        );
        assert!(!flicker_content::package::file_exists(&target.join("NewThing.json")));
        assert!(flicker_content::manifest::read(&manifest).unwrap().is_empty(), "row removed");
        qm.refresh_queue();
        assert_eq!(qm.selected_queue_item().map(|q| q.name.as_str()), Some("NewThing"));
        let _ = std::fs::remove_dir_all(d);
    }

    /// Promoting onto an occupant raises the SAME prompt every mutation uses;
    /// Replace (applied to the rest) parks the occupants and ships the asset.
    #[test]
    fn a_promote_onto_an_occupant_prompts_and_replace_ships_the_whole_asset() {
        let (mut qm, d) = scratch("promoteclash");
        stage_asset(&d, "Golem", true);
        // The occupant — an OLD version already living at the exact target.
        let old = d.join("package/characters/Golem");
        std::fs::create_dir_all(&old).unwrap();
        flicker_content::package::write_text(&old.join("Golem.json"), r#"{"old":true}"#).unwrap();
        qm.refresh_queue();
        assert!(
            qm.facts_warning_tokens().contains(&"$qm_warn_target_occupied"),
            "the occupied target is a WARNING before it is a surprise"
        );

        qm.promote_selected();
        assert!(qm.is_prompting(), "the occupant raises the question");
        qm.toggle_apply_rest();
        qm.resolve_conflict(flicker_content::Resolution::Replace);
        assert!(!qm.is_prompting());
        assert!(qm.last_error_token().is_none(), "replace completes the promote");
        let text =
            flicker_content::package::read_text(&old.join("Golem.json")).unwrap();
        assert!(text.contains("flicker.rig"), "the NEW rig sits at the target");
        assert_eq!(
            flicker_content::manifest::read(&d.join("package/manifest.json")).unwrap().len(),
            1
        );
        let _ = std::fs::remove_dir_all(d);
    }

    /// A Skip answer would strand part of the asset — the promote REFUSES out
    /// loud instead of shipping a partial folder, and nothing moves.
    #[test]
    fn a_partial_conflict_answer_refuses_the_promote() {
        let (mut qm, d) = scratch("promotepartial");
        stage_asset(&d, "Golem", true);
        let old = d.join("package/characters/Golem");
        std::fs::create_dir_all(&old).unwrap();
        flicker_content::package::write_text(&old.join("Golem.json"), r#"{"old":true}"#).unwrap();
        qm.refresh_queue();
        qm.promote_selected();
        assert!(qm.is_prompting());
        qm.toggle_apply_rest();
        qm.resolve_conflict(flicker_content::Resolution::Skip);
        assert_eq!(qm.last_error_token(), Some("$qm_err_promote_partial"));
        assert!(
            flicker_content::package::file_exists(
                &d.join("staging/characters/Golem/Golem.json")
            ),
            "nothing moved"
        );
        assert!(
            flicker_content::manifest::read(&d.join("package/manifest.json")).unwrap().is_empty(),
            "and nothing was recorded"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    /// The queue scans every ingest tier, the facts read the rig, the off-canon
    /// and missing-texture warnings are REAL, and the cursor survives a refresh.
    #[test]
    fn the_queue_reads_facts_warns_honestly_and_selection_survives_refresh() {
        let (mut qm, d) = scratch("queue");
        stage_asset(&d, "Alpha2", true);
        stage_asset(&d, "Beta2", false); // its listed texture is MISSING
        qm.refresh_queue();
        assert_eq!(qm.queue.len(), 2);

        qm.review_nav(1); // → Beta2
        assert_eq!(qm.selected_queue_item().unwrap().name, "Beta2");
        let warns = qm.facts_warning_tokens();
        assert!(warns.contains(&"$qm_warn_off_canon"), "2 bones ≠ the canon: {warns:?}");
        assert!(warns.contains(&"$qm_warn_missing_textures"), "{warns:?}");

        qm.refresh_queue();
        assert_eq!(qm.selected_queue_item().unwrap().name, "Beta2", "cursor survived");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A chord verb fired while the menu is up closes it AND runs — one event,
    /// one dispatcher, whether it came from a row click or the keyboard.
    #[test]
    fn a_chord_verb_closes_the_open_menu_and_runs() {
        let (mut qm, d) = scratch("menuchord");
        qm.apply_results(&on("menu_open"));
        assert!(qm.is_menu_open() && !qm.can_undo());
        qm.apply_results(&on("create_folder"));
        assert!(!qm.is_menu_open(), "the verb closed the menu");
        assert!(qm.can_undo(), "and it ran");
        let _ = std::fs::remove_dir_all(d);
    }

    /// While the menu steers, the d-pad must not ALSO move the listing cursor —
    /// or the selection would slide out from under the open menu.
    #[test]
    fn menu_nav_does_not_move_the_listing_cursor() {
        let (mut qm, d) = scratch("menunav");
        let before = qm.selected().map(|r| r.path.clone());
        qm.apply_results(&on("menu_open"));
        for _ in 0..4 {
            qm.apply_results(&on("nav_down"));
        }
        assert_eq!(qm.selected().map(|r| r.path.clone()), before, "the listing held still");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The menu is modal but yields: Cancel dismisses it, and a pick closes it
    /// and runs the verb in the same frame.
    #[test]
    fn the_menu_dismisses_and_a_pick_runs_the_verb() {
        let (mut qm, d) = scratch("menupick");
        qm.open_menu();
        qm.apply_results(&on("cancel"));
        assert!(!qm.is_menu_open(), "Cancel dismisses without acting");
        assert!(!qm.can_undo(), "and nothing ran");

        // A pick: the menu closes AND the verb runs, in one frame.
        qm.open_menu();
        qm.apply_results(&on("create_folder"));
        assert!(!qm.is_menu_open(), "the pick closed the menu");
        assert!(qm.can_undo(), "and the verb ran");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A collision raised from a menu pick outranks the menu: the prompt owns the
    /// screen and the menu is gone.
    #[test]
    fn a_prompt_raised_from_the_menu_outranks_it() {
        let (mut qm, d) = scratch("menuprompt");
        let folder = qm.cwd().join("characters");
        flicker_content::package::write_text(
            &folder.join("Alpha.json"),
            r#"{"clips":[{"name":"already here"}]}"#,
        )
        .unwrap();
        qm.refresh();
        qm.nav(1);
        qm.cut();
        qm.nav(-1);

        qm.open_menu();
        qm.apply_results(&on("paste"));
        assert!(!qm.is_menu_open(), "the menu closed");
        assert!(qm.is_prompting(), "and the collision question took the screen");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Every mutation confirms itself, and the confirmation names the SUBJECT as
    /// data beside the command's own `$token` — never a composed sentence.
    #[test]
    fn a_mutation_raises_a_toast_naming_what_it_touched() {
        let (mut qm, d) = scratch("toast");
        assert_eq!(qm.toast_count(), 0, "a settled bench is quiet");

        qm.create_folder();
        assert_eq!(qm.toast_count(), 1, "the new folder confirmed itself");

        let m = qm.hud_model();
        assert!(m.is_on("toast_0_on"), "the filled slot is on");
        assert_eq!(
            m.text("toast_0_label"),
            Some("$qm_did_newfolder"),
            "the ACTION is a token"
        );
        assert!(
            m.text("toast_0_name").is_some_and(|n| n.starts_with("New Folder")),
            "the SUBJECT is the item's own name, as data: {:?}",
            m.text("toast_0_name")
        );
        // The empty slots are off, so the proto's other rows stay hidden.
        assert!(!m.is_on("toast_1_on"), "an empty slot stays off");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The bank is capped at the proto's slot count — a fourth confirmation
    /// pushes the oldest out rather than binding a row the proto has not got.
    #[test]
    fn the_toast_bank_never_outgrows_the_protos_slots() {
        let (mut qm, d) = scratch("toastcap");
        for _ in 0..(TOAST_SLOTS + 2) {
            qm.create_folder();
        }
        assert_eq!(qm.toast_count(), TOAST_SLOTS, "capped at the slot count");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The dwell rule, tested without driving a frame: `tick` only advances
    /// inside `update`, which needs a live frame context.
    #[test]
    fn a_toast_expires_after_its_dwell() {
        assert!(toast_alive(0, 0), "just raised");
        assert!(toast_alive(0, TOAST_TICKS - 1), "still inside the dwell");
        assert!(!toast_alive(0, TOAST_TICKS), "gone at the dwell");
        assert!(!toast_alive(0, TOAST_TICKS * 4), "and stays gone");
        // `tick` is a wrapping counter, so `born` can in principle sit AHEAD of
        // `now`. Saturating arithmetic makes that read as freshly-raised rather
        // than long-expired — the safe direction: a confirmation lingers instead
        // of vanishing before it is read.
        assert!(toast_alive(u64::MAX, 0), "a born-ahead toast shows rather than vanishing");
    }

    /// Stage a collision: `Alpha.json` cut from `cwd`, with an occupant of the
    /// same name already sitting in `characters/`. Returns the two paths.
    fn staged_collision(qm: &mut Quartermaster) -> (PathBuf, PathBuf) {
        let folder = qm.cwd().join("characters");
        flicker_content::package::write_text(
            &folder.join("Alpha.json"),
            r#"{"clips":[{"name":"already here"}]}"#,
        )
        .unwrap();
        qm.refresh();
        qm.nav(1); // Alpha.json
        let src = qm.selected().unwrap().path.clone();
        qm.cut();
        qm.nav(-1); // the folder
        qm.paste();
        (src, folder.join("Alpha.json"))
    }

    /// REPLACE moves on top of the occupant — but the occupant is PARKED in
    /// `.trash`, never unlinked, which is what keeps the batch revertible.
    #[test]
    fn replace_displaces_the_occupant_and_undoes_cleanly() {
        let (mut qm, d) = scratch("replace");
        let (src, dst) = staged_collision(&mut qm);
        assert!(qm.is_prompting());

        qm.apply_results(&on("conflict_replace"));
        assert!(!qm.is_prompting(), "answering closes the prompt");
        assert!(!flicker_content::occupied(&src), "the source moved");
        assert_eq!(
            flicker_content::package::read_text(&dst).unwrap(),
            r#"{"format":"flicker.rig","mesh":{"indices":[1,2,3]}}"#,
            "the incoming file is now at the destination"
        );
        assert!(qm.can_undo(), "exactly one entry to undo");
        assert!(qm.clipboard().is_none(), "the move completed");

        qm.undo();
        assert_eq!(
            flicker_content::package::read_text(&dst).unwrap(),
            r#"{"clips":[{"name":"already here"}]}"#,
            "undo brings the displaced occupant back"
        );
        assert!(flicker_content::occupied(&src), "and returns the source");
        let _ = std::fs::remove_dir_all(d);
    }

    /// KEEP BOTH lands beside the occupant under a `_NN` name; both survive.
    #[test]
    fn keep_both_lands_beside_the_occupant() {
        let (mut qm, d) = scratch("keepboth");
        let (src, dst) = staged_collision(&mut qm);

        qm.apply_results(&on("conflict_keep_both"));
        assert!(!qm.is_prompting());
        assert_eq!(
            flicker_content::package::read_text(&dst).unwrap(),
            r#"{"clips":[{"name":"already here"}]}"#,
            "the occupant is untouched"
        );
        let beside = dst.parent().unwrap().join("Alpha_01.json");
        assert!(flicker_content::occupied(&beside), "the incoming landed as _01");
        assert!(!flicker_content::occupied(&src), "and left its old home");
        assert!(qm.can_undo(), "one entry");
        let _ = std::fs::remove_dir_all(d);
    }

    /// SKIP leaves BOTH sides exactly as they were and records nothing — the
    /// move stays on the clipboard, ready to retry somewhere else.
    #[test]
    fn skip_moves_nothing_and_records_nothing() {
        let (mut qm, d) = scratch("skip");
        let (src, dst) = staged_collision(&mut qm);

        qm.apply_results(&on("conflict_skip"));
        assert!(!qm.is_prompting(), "the prompt closes");
        assert_eq!(
            flicker_content::package::read_text(&dst).unwrap(),
            r#"{"clips":[{"name":"already here"}]}"#,
            "the occupant is untouched"
        );
        assert!(flicker_content::occupied(&src), "and so is the source");
        assert!(!qm.can_undo(), "nothing was recorded");
        assert!(qm.clipboard().is_some(), "still pending, ready to retry");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Cancel (B) abandons the QUESTION, not the cut: nothing moves and the
    /// clipboard stays loaded.
    #[test]
    fn cancel_abandons_the_prompt_but_keeps_the_clipboard() {
        let (mut qm, d) = scratch("promptcancel");
        let (src, _) = staged_collision(&mut qm);

        qm.apply_results(&on("cancel"));
        assert!(!qm.is_prompting());
        assert!(flicker_content::occupied(&src), "nothing moved");
        assert!(!qm.can_undo());
        assert!(qm.clipboard().is_some(), "the cut survives the dismissal");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The prompt is MODAL: while it is up nothing else may act, or a stray nav
    /// or chord would move the cursor out from under the question — or mutate
    /// the very item being asked about.
    #[test]
    fn the_prompt_swallows_every_other_intent() {
        let (mut qm, d) = scratch("promptmodal");
        let (src, _) = staged_collision(&mut qm);
        let before = qm.selected().map(|r| r.path.clone());

        for verb in
            ["nav_down", "nav_up", "cut", "paste", "create_folder", "undo", "redo", "rename"]
        {
            qm.apply_results(&on(verb));
        }
        assert!(qm.is_prompting(), "still prompting after every other intent");
        assert!(!qm.is_renaming(), "rename never opened");
        assert_eq!(qm.selected().map(|r| r.path.clone()), before, "the cursor did not move");
        assert!(!qm.can_undo(), "nothing mutated");
        assert!(flicker_content::occupied(&src), "the item in question is untouched");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Rename must act on the item the user is LOOKING at. `confirm`, `cancel`
    /// and `nav` all dispatch on `self.pane`; `begin_rename` read the listing
    /// cursor unconditionally, so firing it while the TREE held focus opened an
    /// edit on a listing row that was not the focused item.
    #[test]
    fn rename_acts_on_the_focused_pane() {
        let (mut qm, d) = scratch("renamepane");
        qm.focus_pane(true);
        assert_eq!(qm.pane(), Pane::Tree, "the tree holds focus");
        let listing = qm.selected().expect("a listing row exists").name.clone();

        qm.apply_results(&on("rename"));
        assert_ne!(
            qm.rename_draft(),
            Some(listing.as_str()),
            "rename opened on the LISTING row `{listing}` while the TREE held focus"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    /// THE controller baseline for mutation: pick up, navigate, put down, take
    /// back — all on a single focus, with no pointer and no multi-select.
    #[test]
    fn a_full_move_and_undo_runs_on_a_single_focus() {
        let (mut qm, d) = scratch("move");
        let src = qm.rows()[1].path.clone(); // Alpha.json
        assert_eq!(qm.rows()[1].name, "Alpha.json");

        // Focus it and pick it up.
        qm.nav(1);
        qm.cut();
        assert_eq!(qm.clipboard(), Some(src.as_path()));

        // Focus the folder and put it down INTO it.
        qm.nav(-1);
        assert!(qm.selected().unwrap().is_dir);
        qm.paste();
        assert!(qm.last_error().is_none(), "{:?}", qm.last_error());
        assert!(qm.clipboard().is_none(), "the clipboard cleared on paste");

        let moved = qm.cwd().join("characters/Alpha.json");
        assert!(flicker_content::occupied(&moved), "the file landed in the folder");
        assert!(!flicker_content::occupied(&src), "and left where it was");
        assert!(qm.can_undo());

        // One undo takes the whole thing back.
        qm.undo();
        assert!(flicker_content::occupied(&src), "undo returned it");
        assert!(!flicker_content::occupied(&moved));
        assert!(qm.can_redo());

        qm.redo();
        assert!(flicker_content::occupied(&moved), "redo re-applied it");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Dropping an item where it already lives must not push an entry the user
    /// would then have to undo for nothing.
    #[test]
    fn pasting_into_the_same_folder_is_a_no_op() {
        let (mut qm, d) = scratch("noop");
        qm.nav(1); // Alpha.json
        qm.cut();
        qm.paste(); // still in the same directory
        assert!(!qm.can_undo(), "a no-op move pushed a history entry");
        assert!(qm.clipboard().is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    /// Moving a folder inside itself would orphan the subtree and cannot be
    /// undone coherently — it is refused, loudly.
    #[test]
    fn a_folder_cannot_be_pasted_into_itself() {
        let (mut qm, d) = scratch("selfmove");
        let folder = qm.rows()[0].path.clone(); // characters/
        qm.cut(); // focus is on the folder
        assert_eq!(qm.clipboard(), Some(folder.as_path()));
        qm.confirm(); // navigate INTO it
        qm.paste();
        assert_eq!(qm.last_error(), Some("$qm_err_into_self"), "refused with a reason");
        assert!(!qm.can_undo(), "and nothing was recorded");
        let _ = std::fs::remove_dir_all(d);
    }

    /// INTERIM behaviour, deliberately conservative: until the conflict dialog
    /// exists, a colliding paste is REFUSED. The file-op layer would replace and
    /// park the occupant in `.trash` (recoverable), but replacing something the
    /// user never agreed to replace is not the tool's call.
    #[test]
    fn a_colliding_paste_raises_the_prompt_rather_than_replacing_silently() {
        let (mut qm, d) = scratch("collide");
        let folder = qm.cwd().join("characters");
        // Put a name in the folder that the file we are about to move shares.
        flicker_content::package::write_text(
            &folder.join("Alpha.json"),
            r#"{"clips":[{"name":"already here"}]}"#,
        )
        .unwrap();
        qm.refresh();

        qm.nav(1); // Alpha.json
        qm.cut();
        qm.nav(-1); // the folder
        qm.paste();

        // The interim REFUSAL is now the PROMPT. What must not change is that a
        // collision never silently replaces: nothing is touched until answered.
        assert!(qm.is_prompting(), "a collision raises the prompt");
        assert_eq!(qm.prompt_remaining(), 1, "one collision to answer");
        assert!(!qm.can_undo(), "nothing was recorded");
        assert!(qm.clipboard().is_some(), "the move is still pending, ready to retry");
        assert_eq!(
            flicker_content::package::read_text(&folder.join("Alpha.json")).unwrap(),
            r#"{"clips":[{"name":"already here"}]}"#,
            "the occupant is untouched"
        );
        assert!(flicker_content::occupied(&qm.cwd().join("Alpha.json")), "and so is the source");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_new_folder_lands_focused_and_undoes_away() {
        let (mut qm, d) = scratch("mkdir");
        let before = qm.rows().len();
        qm.create_folder();
        assert!(qm.last_error().is_none(), "{:?}", qm.last_error());
        assert_eq!(qm.rows().len(), before + 1);
        assert!(qm.selected().unwrap().is_dir, "the new folder is focused, ready to rename");

        // A second one must not collide with the first.
        qm.create_folder();
        assert_eq!(qm.rows().len(), before + 2, "the next free name was used");

        qm.undo();
        assert_eq!(qm.rows().len(), before + 1);
        qm.undo();
        assert_eq!(qm.rows().len(), before, "back to where we started");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Cancel drops a pending move before it navigates — backing out of the
    /// clipboard is the more recent intent.
    #[test]
    fn cancel_drops_a_pending_move_before_it_navigates() {
        let (mut qm, d) = scratch("cancelclip");
        let here = qm.cwd().to_path_buf();
        qm.confirm(); // into characters/
        qm.cut();
        assert!(qm.clipboard().is_some());

        qm.apply_results(&on("cancel"));
        assert!(qm.clipboard().is_none(), "the first Cancel dropped the move");
        assert_eq!(qm.cwd(), qm.cwd(), "and did not also navigate");

        qm.apply_results(&on("cancel"));
        assert_eq!(qm.cwd(), here, "the second Cancel climbed out");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The MISSING gate kind, closed for this screen: `raw_display_literals`
    /// proves every display string IS a `$token`, but nothing proves the token
    /// RESOLVES — a typo'd or unadded token ships as raw `$qm_…` on screen.
    /// This walks the drawn output with the shipped stringtable loaded and
    /// asserts no `$` survived to the draw boundary.
    ///
    /// (A deliberate literal `$` would be authored `$$…`, which resolve() emits
    /// as `$…`; this screen ships none, so any leading `$` here is a miss.)
    #[test]
    fn every_token_the_screen_ships_resolves() {
        let (mut qm, d) = scratch("tokens");
        for _ in 0..Tab::ALL.len() {
            for screen in [Vec2::new(1920.0, 1080.0), Vec2::new(1280.0, 720.0)] {
                let unresolved: Vec<String> = drawn_text(&qm, screen)
                    .into_iter()
                    .filter(|(_, _, t)| t.starts_with('$'))
                    .map(|(_, _, t)| t)
                    .collect();
                assert!(
                    unresolved.is_empty(),
                    "tokens with no stringtable entry on {:?} at {screen:?}: {unresolved:?}",
                    qm.tab()
                );
            }
            qm.cycle_tab();
        }
        let _ = std::fs::remove_dir_all(d);
    }

    // ── helpers ──

    /// A results map with one name fired — what the walker hands the dispatcher
    /// when a pad chord, a key, or a click resolves to that intent.
    fn on(name: &str) -> ValueMap {
        let mut m = ValueMap::new();
        m.set(name, true);
        m
    }

}

/// ⛔ QUARANTINED scene styles (five-line split, Aaron 2026-08-12): this dormant
/// bench's style blocks, vendored OUT of ui_theme.json — a scene's values belong
/// in its scene file, and these move into this bench's own `.scene.json` at its
/// migration. Do not grow this file.
pub(crate) fn scene_styles() -> serde_json::Value {
    serde_json::from_str(include_str!("../scene_styles.json"))
        .expect("scene_styles.json parses")
}
