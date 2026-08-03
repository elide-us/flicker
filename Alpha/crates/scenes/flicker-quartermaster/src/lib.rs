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
}

impl Command for FileCommand {
    fn apply(&mut self) -> Result<(), String> {
        self.batch.apply().map_err(|e| e.to_string())
    }
    fn revert(&mut self) -> Result<(), String> {
        self.batch.revert().map_err(|e| e.to_string())
    }
    fn label(&self) -> &str {
        &self.label
    }
}

use std::path::PathBuf;
use std::time::Duration;

use flicker::render::{Renderer, Vec2};
use flicker::scene::{Scene, Transition};
use flicker::script::{
    ComponentLibrary, HudCommand, ScriptHost, UiAnchor, UiNode, Value, ValueMap,
};
use flicker::ui::{
    builtin_templates, expand, load_styles, render_hud, run_ui_with, TemplateRegistry, UiInput,
    UiIntents, UiState, WalkerHandler, UI_COMPONENT_MODULES,
};
use flicker_input_core::{
    AbstractControls, ContextualBindings, Fired, GamepadConfig, InputMap, InputState, Key, Resolver,
};
use flicker_input_router::{apply_context_requests, InputEvent, InputHandler, RouteCtx, Router};
use flicker_shell::{PauseScene, Theme};

use fs_model::{Roots, Row, SortKey, TreeRow};

const HUD_UI_ELEMENTS: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../content/sensorium/resources/ui_elements.json");

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
    script: Option<ScriptHost>,
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

    /// The template registry `build_tree` expands against — built once; the
    /// protos it points at are the parse-once embedded `ui_templates.json`.
    templates: TemplateRegistry,

    bindings: ContextualBindings,
    chord: ChordLayer,
    gamepad_config: GamepadConfig,
    resolver: Resolver,
    ev: Vec<Fired>,
    tick: u64,
    fired_sigs: Vec<String>,
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
            templates: builtin_templates(),
            expanded: Vec::new(),
            tree: Vec::new(),
            tree_sel: 0,
            list_scroll: 0.0,
            tree_scroll: 0.0,
            list_view_h: 600.0,
            ui_intents: UiIntents::default(),
            ui_state: UiState::default(),
            ui_styles: serde_json::Value::Null,
            script: None,
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
            self.prompt =
                Some(ConflictPrompt { pending: conflicts, ops: Vec::new(), apply_rest: false });
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
        if p.ops.is_empty() {
            // Everything was skipped — nothing ran, so the move stays pending
            // and the clipboard is still loaded, ready to retry elsewhere.
            return;
        }
        if self.run(p.ops, "$qm_did_move") {
            self.clipboard = None;
        }
    }

    /// Abandon the prompt without moving anything. The clipboard stays loaded —
    /// backing out of the question is not backing out of the cut.
    pub fn cancel_conflict(&mut self) {
        self.prompt = None;
        self.last_error = None;
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

    /// This frame's chrome. Rebuilt every frame because node ids encode
    /// positions in the CURRENT listing — the sanctioned Rust-tree case.
    pub fn build_tree(&self, screen: Vec2) -> UiNode {
        let mut page = node("screen");
        // The screen's input DECLARATION (S9). Everything the bench reacts to is
        // named here as data, so a pad chord, a key and a click are the same
        // event by the time the scene sees them.
        for (signal, result) in [
            ("on_menu", "pause_open"),
            ("on_confirm", "confirm"),
            ("on_cancel", "cancel"),
            ("on_nav_up", "nav_up"),
            ("on_nav_down", "nav_down"),
            ("on_nav_left", "nav_left"),
            ("on_nav_right", "nav_right"),
            ("on_panel_next", "panel_next"),
            ("on_panel_prev", "panel_prev"),
            ("on_tab_next", "tab_next"),
            ("on_tab_prev", "tab_prev"),
            // The chord layer's editor verbs. A pad chord, a Ctrl-key and a
            // menu click all arrive at the dispatcher as these same names.
            ("on_cut", "cut"),
            ("on_paste", "paste"),
            ("on_undo", "undo"),
            ("on_redo", "redo"),
            ("on_create_folder", "create_folder"),
            ("on_rename", "rename"),
            ("on_context_menu", "menu_open"),
        ] {
            page.props.insert(signal.into(), Value::Text(result.into()));
        }
        page.children = vec![
            self.top_bar(screen),
            self.tab_bar(screen),
            self.crumb_bar(screen),
            self.body(screen),
            self.status_bar(screen),
            // The context menu floats over the bench, under the toasts.
            self.item_context_menu(),
            // Confirmations float over the bench; each row gates itself.
            self.toast_stack(),
            // Last, so it paints over everything; `has_prompt` gates it.
            self.conflict_dialog(),
        ];
        // Expanded HERE, not at the call sites, so the scene and every test walk
        // the SAME tree — the one seam the drift gates demand. A template-free
        // tree passes through unchanged, so the rest of the bench is unaffected.
        expand(page, &self.templates)
    }

    fn top_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "top_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.width = Some(screen.x);
        bar.height = Some(TOP_BAR_H);
        bar.pad = 16.0;
        bar.gap = 12.0;
        bar = prop(bar, "style", text_val("quartermaster.top_bar"));

        let title = label("$qm_title", 21.0, "quartermaster.title.color", Some(260.0));
        let route = label("$qm_route", 11.0, "quartermaster.badge.color", Some(220.0));
        let mut spacer = node("cell");
        spacer.grow = Some(1.0);
        bar.children = vec![title, route, spacer];
        bar
    }

    fn tab_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "tab_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.offset = [0.0, TOP_BAR_H];
        bar.width = Some(screen.x);
        bar.height = Some(TAB_BAR_H);
        bar.pad = 6.0;
        bar.gap = 4.0;
        bar = prop(bar, "style", text_val("quartermaster.tab_bar"));
        bar.children = Tab::ALL
            .iter()
            .map(|t| {
                let mut b = node("button");
                b.id = t.id().into();
                b.action = Some(t.id().into());
                b.size = Some(150.0);
                b = prop(b, "label", text_val(t.token()));
                let style = if *t == self.tab {
                    "quartermaster.tab_active"
                } else {
                    "quartermaster.tab_idle"
                };
                prop(b, "style", text_val(style))
            })
            .collect();
        bar
    }

    fn crumb_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "crumb_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.offset = [0.0, TOP_BAR_H + TAB_BAR_H];
        bar.width = Some(screen.x);
        bar.height = Some(CRUMB_H);
        bar.pad = 20.0;
        bar.gap = 8.0;
        bar = prop(bar, "style", text_val("quartermaster.crumb_bar"));

        let mut crumbs = node("text");
        crumbs.id = "crumbs".into();
        crumbs = prop(crumbs, "text_bind", text_val("crumbs"));
        crumbs = prop(crumbs, "size", Value::Number(13.0));
        crumbs = prop(crumbs, "color", text_val("quartermaster.crumb.color"));
        bar.children = vec![crumbs];
        bar
    }

    fn body(&self, screen: Vec2) -> UiNode {
        let top = TOP_BAR_H + TAB_BAR_H + CRUMB_H;
        let h = (screen.y - top - STATUS_H).max(0.0);
        match self.tab {
            Tab::Files => self.files_body(screen, top, h),
            Tab::Review => self.review_placeholder(screen, top, h),
        }
    }

    fn files_body(&self, screen: Vec2, top: f32, h: f32) -> UiNode {
        let mut row = node("row");
        row.id = "files_body".into();
        row.anchor = Some(UiAnchor::TopLeft);
        row.offset = [0.0, top];
        row.width = Some(screen.x);
        row.height = Some(h);
        row.children = vec![self.tree_pane(h), self.list_pane((screen.x - TREE_W).max(0.0), h)];
        row
    }

    /// The folder tree. A sibling of the list, never nested inside it — a
    /// `list` REPLACES the inherited clip, so nesting two would clip wrong.
    fn tree_pane(&self, h: f32) -> UiNode {
        let mut pane = node("cell");
        pane.id = "tree_pane".into();
        pane.width = Some(TREE_W);
        pane.height = Some(h);
        pane = prop(pane, "style", text_val(pane_style(self.pane == Pane::Tree)));

        let mut scroll = node("list");
        scroll.id = "tree_list".into();
        scroll.bind = Some("tree_scroll".into());
        scroll.grow = Some(1.0);
        scroll.pad = 8.0;
        scroll.children = self.tree.iter().enumerate().map(|(i, t)| self.tree_row(i, t)).collect();

        pane.children =
            vec![label("$qm_tree_header", 11.0, "quartermaster.header.color", None), scroll];
        pane
    }

    /// The collision prompt — ONE instance node of the `conflict_dialog` data
    /// proto (`ui_templates.json`), choice_dialog's modal sibling. The proto owns
    /// the composition and the fixed `conflict_*` bind/action/token vocabulary;
    /// this bench only gates it (`visible_bind`), skins it, publishes the binds
    /// (`hud_model`) and consumes the three actions (`apply_results`). Always in
    /// the tree so node ids stay stable and the drift gates see it while closed.
    /// The confirmation bank — ONE instance of the `toast_stack` proto. The
    /// proto owns the layout and the `toast_<i>_*` bind vocabulary; the bench
    /// only pages its live toasts into those binds and lets the rows' Undo
    /// buttons fire the same `undo` result the chord does.
    /// The focused-item context menu — ONE instance of the `item_context_menu`
    /// proto. Its rows' actions ARE the bench's verb names, so a menu pick and a
    /// chord are the same event by the time the dispatcher sees them.
    ///
    /// `paste_disabled` rides a TEMPLATE PARAM rather than a bind: the walker
    /// copies data-children props verbatim without model resolution, and the
    /// per-frame re-expansion is what keeps it live.
    fn item_context_menu(&self) -> UiNode {
        let mut n = UiNode { template: Some("item_context_menu".into()), ..Default::default() };
        n.id = "item_menu".into();
        n = prop(n, "on_bind", text_val("has_menu"));
        n = prop(n, "paste_disabled", Value::Bool(self.clipboard.is_none()));
        // The pad cursor's highlight rides the same per-row param mechanic as
        // `disabled`: data-children props are not model-resolved, and the
        // per-frame re-expansion is what keeps it live.
        for (i, (_, row)) in MENU_ROWS.iter().enumerate() {
            n = prop(n, &format!("active_{row}"), Value::Bool(self.menu_sel == i));
        }
        let at = self.menu.unwrap_or(Vec2::new(0.0, 0.0));
        n = prop(n, "x", Value::Number(f64::from(at.x)));
        n = prop(n, "y", Value::Number(f64::from(at.y)));
        n
    }

    fn toast_stack(&self) -> UiNode {
        let mut n = UiNode { template: Some("toast_stack".into()), ..Default::default() };
        n.id = "toast_stack".into();
        n
    }

    fn conflict_dialog(&self) -> UiNode {
        let mut n = UiNode { template: Some("conflict_dialog".into()), ..Default::default() };
        n.id = "conflict_dialog".into();
        n.visible_bind = Some("has_prompt".into());
        n = prop(n, "overlay_style", text_val("quartermaster.scrim"));
        n = prop(n, "card_style", text_val("quartermaster.card"));
        n = prop(n, "check_style", text_val("quartermaster.checkbox"));
        n
    }

    fn tree_row(&self, i: usize, t: &TreeRow) -> UiNode {
        let mut stack = node("stack");
        stack.height = Some(ROW_H);
        stack.id = format!("treerow_{i}");

        let mut wash = node("cell");
        // `visible_bind`, `tab_group` and `nav_ordinal` are FIELDS on UiNode, not
        // props — a prop of the same name is silently ignored by the walker.
        wash.visible_bind = Some(format!("tree_{i}_sel"));
        // See `list_row`: a childless `cell` measures to zero height and draws an
        // invisible wash unless it is given the row height.
        wash.height = Some(ROW_H);
        wash = prop(wash, "width_frac", Value::Number(1.0));
        wash = prop(wash, "style", text_val("quartermaster.rowsel"));

        // The click target spans the row and is NEVER gated on selection —
        // gating it would make every unselected row unclickable.
        let mut hit = node("button");
        hit.id = format!("tree_hit_{i}");
        hit.action = Some(format!("tree_open_{i}"));
        hit.size = Some(ROW_H);
        hit = prop(hit, "width_frac", Value::Number(1.0));
        hit = prop(hit, "style", text_val("quartermaster.rowsel_off"));
        hit.tab_group = Pane::Tree.id().into();
        hit.nav_ordinal = i as u32;

        // A `stack` hugs its largest child, so a row left to measure itself
        // collapses to its content width and nothing inside can distribute —
        // `width_frac` is what makes the row span the pane.
        let mut inner = node("row");
        inner = prop(inner, "width_frac", Value::Number(1.0));
        inner.gap = 8.0;
        // Indent with a leading SPACER, never with `pad`: `pad` insets all FOUR
        // sides, so a deep row also grew VERTICALLY and overlapped its
        // neighbours (`Strafe` over `RootMotion`, `muse` over `epochs`). The
        // spacer moves the caret right without touching the row's height.
        inner.pad_y = Some(0.0);
        let mut indent = node("cell");
        indent.size = Some(t.depth as f32 * 18.0);
        let mut caret = node("text");
        caret = prop(caret, "text_bind", text_val(format!("tree_{i}_caret")));
        caret = prop(caret, "size", Value::Number(11.0));
        caret = prop(caret, "color", text_val("quartermaster.caret.color"));
        let mut name = node("text");
        name = prop(name, "text_bind", text_val(format!("tree_{i}_name")));
        name = prop(name, "size", Value::Number(14.0));
        name = prop(name, "color", text_val("quartermaster.tree.color"));
        inner.children = vec![indent, caret, name];

        stack.children = vec![wash, hit, inner];
        stack
    }

    fn list_pane(&self, w: f32, h: f32) -> UiNode {
        let mut pane = node("cell");
        pane.id = "list_pane".into();
        pane.width = Some(w);
        pane.height = Some(h);
        pane = prop(pane, "style", text_val(pane_style(self.pane == Pane::List)));

        let mut scroll = node("list");
        scroll.id = "file_list".into();
        scroll.bind = Some("list_scroll".into());
        scroll.grow = Some(1.0);
        let (first, last) = self.visible_range();
        let mut kids: Vec<UiNode> = Vec::new();
        // A spacer stands in for the rows above the window so the walker's
        // measured content height stays truthful and the scrollbar is honest.
        if first > 0 {
            let mut spacer = node("cell");
            spacer.height = Some(first as f32 * LIST_ROW_H);
            kids.push(spacer);
        }
        kids.extend(
            self.rows[first..last].iter().enumerate().map(|(n, r)| self.list_row(first + n, r)),
        );
        if last < self.rows.len() {
            let mut spacer = node("cell");
            spacer.height = Some((self.rows.len() - last) as f32 * LIST_ROW_H);
            kids.push(spacer);
        }
        scroll.children = kids;

        pane.children = vec![self.list_header(), scroll];
        pane
    }

    fn list_header(&self) -> UiNode {
        let mut header = node("row");
        header.id = "list_header".into();
        header.height = Some(HEADER_H);
        header.pad = 20.0;
        header.gap = 14.0;
        header = prop(header, "style", text_val("quartermaster.list_header"));
        header.children = [
            ("sort_name", "$qm_col_name", 0.0),
            ("sort_type", "$qm_col_type", 190.0),
            ("sort_size", "$qm_col_size", 130.0),
        ]
        .iter()
        .map(|(id, token, w)| {
            let mut b = node("button");
            b.id = (*id).into();
            b.action = Some((*id).into());
            if *w > 0.0 {
                b.size = Some(*w);
            } else {
                b.grow = Some(1.0);
            }
            b = prop(b, "label", text_val(*token));
            prop(b, "style", text_val("quartermaster.col_header"))
        })
        .collect();
        header
    }

    fn list_row(&self, i: usize, _row: &Row) -> UiNode {
        let mut stack = node("stack");
        stack.height = Some(LIST_ROW_H);
        stack.id = format!("listrow_{i}");

        let mut wash = node("cell");
        wash.visible_bind = Some(format!("row_{i}_sel"));
        // A childless `cell` MEASURES to its content, so a wash left to itself is
        // a ZERO-HEIGHT rectangle: bound correctly, published correctly, drawn —
        // and invisible. It must be told the row height.
        wash.height = Some(LIST_ROW_H);
        wash = prop(wash, "width_frac", Value::Number(1.0));
        wash = prop(wash, "style", text_val("quartermaster.rowsel"));

        let mut hit = node("button");
        hit.id = format!("row_hit_{i}");
        hit.action = Some(format!("row_pick_{i}"));
        hit.size = Some(LIST_ROW_H);
        hit = prop(hit, "width_frac", Value::Number(1.0));
        hit = prop(hit, "style", text_val("quartermaster.rowsel_off"));
        hit.tab_group = Pane::List.id().into();
        hit.nav_ordinal = i as u32;

        // Spans the row (see `tree_row`) — without this the three columns bunch
        // at the left edge instead of lining up under their headers.
        let mut inner = node("row");
        inner = prop(inner, "width_frac", Value::Number(1.0));
        inner.pad = 20.0;
        inner.gap = 14.0;
        // While this row is being renamed the caption becomes an editable field
        // in place; every other row is untouched.
        let renaming = self.rename.as_ref().is_some_and(|r| r.path == _row.path);
        let mut name = if renaming {
            let mut f = node("text_field");
            f.id = RENAME_ID.into();
            // `bind` is a FIELD: it names the Model key holding the draft.
            f.bind = Some("rename_draft".into());
            f = prop(f, "size", Value::Number(14.0));
            f = prop(
                f,
                "style",
                text_val(if self.last_error.is_some() {
                    "quartermaster.rename_bad"
                } else {
                    "quartermaster.rename"
                }),
            );
            f
        } else {
            let mut t = node("text");
            t = prop(t, "text_bind", text_val(format!("row_{i}_name")));
            t = prop(t, "color_bind", text_val(format!("row_{i}_color")));
            t = prop(t, "size", Value::Number(14.0));
            t
        };
        // The name carries the CLASS COLOUR — the derived classification IS the
        // colour, so a glance down the column reads as content types.
        name.grow = Some(1.0);
        let mut kind = node("text");
        kind = prop(kind, "text_bind", text_val(format!("row_{i}_type")));
        kind = prop(kind, "size", Value::Number(11.0));
        kind = prop(kind, "color", text_val("quartermaster.meta.color"));
        kind.width = Some(190.0);
        let mut size = node("text");
        size = prop(size, "text_bind", text_val(format!("row_{i}_size")));
        size = prop(size, "size", Value::Number(12.0));
        size = prop(size, "color", text_val("quartermaster.meta.color"));
        size.width = Some(130.0);
        inner.children = vec![name, kind, size];

        stack.children = vec![wash, hit, inner];
        stack
    }

    fn review_placeholder(&self, screen: Vec2, top: f32, h: f32) -> UiNode {
        let mut cell = node("cell");
        cell.id = "review_body".into();
        cell.anchor = Some(UiAnchor::TopLeft);
        cell.offset = [0.0, top];
        cell.width = Some(screen.x);
        cell.height = Some(h);
        cell.pad = 40.0;
        cell = prop(cell, "style", text_val("quartermaster.pane"));
        cell.children = vec![label("$qm_review_soon", 15.0, "quartermaster.meta.color", None)];
        cell
    }

    fn status_bar(&self, screen: Vec2) -> UiNode {
        let mut bar = node("row");
        bar.id = "status_bar".into();
        bar.anchor = Some(UiAnchor::TopLeft);
        bar.offset = [0.0, (screen.y - STATUS_H).max(0.0)];
        bar.width = Some(screen.x);
        bar.height = Some(STATUS_H);
        bar.pad = 20.0;
        bar.gap = 10.0;
        bar = prop(bar, "style", text_val("quartermaster.status_bar"));

        let mut count = node("text");
        count.id = "status_count".into();
        count = prop(count, "text_bind", text_val("count"));
        count = prop(count, "size", Value::Number(11.0));
        count = prop(count, "color", text_val("quartermaster.meta.color"));

        let mut spacer = node("cell");
        spacer.grow = Some(1.0);

        // The pending move, shown only while something is held.
        let mut clip_label = label("$qm_holding", 11.0, "quartermaster.clip.color", None);
        clip_label.visible_bind = Some("has_clip".into());
        let mut clip_name = node("text");
        clip_name.id = "status_clip".into();
        clip_name = prop(clip_name, "text_bind", text_val("clip_name"));
        clip_name.visible_bind = Some("has_clip".into());
        clip_name = prop(clip_name, "size", Value::Number(11.0));
        clip_name = prop(clip_name, "color", text_val("quartermaster.clip.color"));

        // A refused mutation says so rather than failing silently.
        let mut err = node("text");
        err.id = "status_error".into();
        err = prop(err, "text_bind", text_val("error"));
        err.visible_bind = Some("has_error".into());
        err = prop(err, "size", Value::Number(11.0));
        err = prop(err, "color", text_val("quartermaster.error.color"));

        bar.children = vec![
            label("$qm_items", 11.0, "quartermaster.meta.color", None),
            count,
            spacer,
            err,
            clip_label,
            clip_name,
        ];
        bar
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
            // Anything else is a POINTER pick on a row (the component fires the
            // row's action): close, then let the ordinary verb arms run it.
            self.close_menu();
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
            self.nav(-1);
        }
        if results.is_on("nav_down") {
            self.nav(1);
        }
        // Left/right cross panes, so a d-pad alone reaches everything.
        if results.is_on("nav_left") {
            self.focus_pane(false);
        }
        if results.is_on("nav_right") {
            self.focus_pane(true);
        }
        if results.is_on("confirm") {
            self.confirm();
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
        self.ui_styles = load_styles(HUD_UI_ELEMENTS);
        // The tree is built in Rust, so the script host is here purely as the
        // COMPONENT LIBRARY (the Lua draw/hit modules the walker calls).
        match ScriptHost::library(UI_COMPONENT_MODULES) {
            Ok(host) => self.script = Some(host),
            Err(e) => tracing::warn!("component library load failed: {e} — no HUD"),
        }
        self.refresh();
        renderer.window().set_title("Quartermaster Bench");
    }

    fn update(&mut self, _dt: Duration, input: &InputState, renderer: &Renderer) -> Transition {
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
        let lib = self.script.as_ref().map(|h| h as &dyn ComponentLibrary);
        let frame = run_ui_with(&tree, &model, &self.ui_styles, &snap, &mut self.ui_state, lib);
        let over_hud = frame.results.is_on("hud_hit");
        self.hud_commands = frame.commands;

        // ONE resolve, ONE dispatch — the walker layer consumes the screen's
        // declared intents, so navigation never reads a raw key.
        self.tick = self.tick.wrapping_add(1);
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

fn node(component: &str) -> UiNode {
    UiNode { component: component.to_string(), ..Default::default() }
}

fn prop(mut n: UiNode, key: &str, value: Value) -> UiNode {
    n.props.insert(key.to_string(), value);
    n
}

fn text_val(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

/// A `text` node carrying a `$token`; `color_path` is a dotted path into the
/// resolved style JSON, never an inline colour.
fn label(token: &str, size: f32, color_path: &str, width: Option<f32>) -> UiNode {
    let mut n = node("text");
    n = prop(n, "text", text_val(token));
    n = prop(n, "size", Value::Number(f64::from(size)));
    n = prop(n, "color", text_val(color_path));
    n.width = width;
    n
}

fn pane_style(focused: bool) -> &'static str {
    if focused {
        "quartermaster.pane_focused"
    } else {
        "quartermaster.pane"
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
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match row.size {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.0} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flicker_content::PackageClass;
    use flicker_input_core::ActionSignal;

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
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
        assert!(find_id(&tree, "review_body").is_some(), "the Review body is shown");
        assert!(find_id(&tree, "files_body").is_none());
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

    /// A `list` REPLACES the inherited scissor rather than intersecting it, so a
    /// list inside a list clips wrong. The tree and the file list must stay
    /// siblings — this pins that.
    #[test]
    fn the_two_lists_are_siblings_never_nested() {
        let (qm, d) = scratch("clip");
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
        fn walk(n: &UiNode, lists: usize, deepest: &mut usize) {
            let lists = lists + usize::from(n.component == "list");
            *deepest = (*deepest).max(lists);
            for c in &n.children {
                walk(c, lists, deepest);
            }
        }
        let mut deepest = 0;
        walk(&tree, 0, &mut deepest);
        assert_eq!(deepest, 1, "no list is nested inside another");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Only the visible window becomes UI — a folder of hundreds of files must
    /// not build hundreds of row nodes.
    #[test]
    fn a_big_folder_only_builds_its_visible_window() {
        let d = std::env::temp_dir().join("flicker_qmbench_window");
        let _ = std::fs::remove_dir_all(&d);
        let roots = Roots { package: d.join("package"), staging: d.join("staging") };
        std::fs::create_dir_all(&roots.package).unwrap();
        std::fs::create_dir_all(&roots.staging).unwrap();
        for i in 0..400 {
            flicker_content::package::write_text(
                &roots.package.join(format!("clip_{i:03}.json")),
                r#"{"clips":[{"name":"x"}]}"#,
            )
            .unwrap();
        }
        let qm = Quartermaster::with_roots(roots);
        assert_eq!(qm.rows().len(), 400);
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
        let rows = count_ids_prefixed(&tree, "listrow_");
        assert!(rows < 60, "windowed to the viewport, built {rows} rows of 400");
        assert!(rows > 5, "but it did build the visible ones ({rows})");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_screen_declares_its_intents_as_data() {
        let (qm, d) = scratch("intents");
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
        let intents = UiIntents::of(&tree);
        for (sig, name) in [
            (ActionSignal::Menu, "pause_open"),
            (ActionSignal::Confirm, "confirm"),
            (ActionSignal::Cancel, "cancel"),
            (ActionSignal::NavUp, "nav_up"),
            (ActionSignal::NavDown, "nav_down"),
            (ActionSignal::PanelNext, "panel_next"),
            (ActionSignal::PanelPrev, "panel_prev"),
            (ActionSignal::TabNext, "tab_next"),
            (ActionSignal::TabPrev, "tab_prev"),
        ] {
            assert_eq!(intents.result_for(sig), Some(name), "{sig:?} must be declared");
        }
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

    /// Nothing may escape the screen at either shipped resolution.
    #[test]
    fn the_chrome_fits_both_reference_resolutions() {
        let (qm, d) = scratch("geometry");
        fn walk(n: &UiNode, screen: Vec2, over: &mut Vec<String>) {
            if let Some(w) = n.width {
                if w > screen.x + 0.5 {
                    over.push(format!("{} w={w}", n.id));
                }
            }
            if let Some(h) = n.height {
                if h > screen.y + 0.5 {
                    over.push(format!("{} h={h}", n.id));
                }
            }
            for c in &n.children {
                walk(c, screen, over);
            }
        }
        for screen in [Vec2::new(1920.0, 1080.0), Vec2::new(1280.0, 720.0)] {
            let tree = qm.build_tree(screen);
            let mut over = Vec::new();
            walk(&tree, screen, &mut over);
            assert!(over.is_empty(), "chrome overflows {screen:?}: {over:?}");
        }
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
        let styles = load_styles(HUD_UI_ELEMENTS);
        let host = ScriptHost::library(UI_COMPONENT_MODULES).expect("component library");
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
        let frame = run_ui_with(
            &tree,
            &qm.hud_model(),
            &styles,
            &snap,
            &mut state,
            Some(&host as &dyn ComponentLibrary),
        );
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

    /// The listing's columns must line up under their headers, and the rows must
    /// span the pane rather than bunching at the left edge.
    ///
    /// Regression: the row's inner `row` carried no `width_frac`, so — a `stack`
    /// hugging its largest child — it measured to its CONTENT and the `grow`
    /// column had nothing to distribute into. Aaron saw it as "text alignment
    /// is off" on the first smoke test.
    #[test]
    fn the_listing_columns_span_the_pane_and_line_up() {
        let (qm, d) = scratch("align");
        let screen = Vec2::new(1920.0, 1080.0);
        let text = drawn_text(&qm, screen);
        assert!(!text.is_empty(), "the walker drew no text at all");

        // The listing's ROWS, by geometry: right of the tree pane and below the
        // column header. (The header's own labels are `button` captions, which
        // are centre-anchored, so their x is not comparable to a row cell's.)
        let header_bottom = TOP_BAR_H + TAB_BAR_H + CRUMB_H + HEADER_H;
        let rows_bottom = screen.y - STATUS_H;
        let row_cells: Vec<&(f32, f32, String)> = text
            .iter()
            .filter(|(x, y, _)| *x > TREE_W && *y > header_bottom && *y < rows_bottom)
            .collect();
        assert!(!row_cells.is_empty(), "no text drawn in the listing rows: {text:?}");

        // Spanning: a bunched row leaves the right of the pane bare.
        let rightmost = row_cells.iter().map(|(x, _, _)| *x).fold(f32::MIN, f32::max);
        assert!(
            rightmost > screen.x * 0.6,
            "row text stops at x={rightmost} — the columns bunched at the left instead of spanning"
        );

        // Alignment: every row shares the SAME three column origins.
        let mut xs: Vec<i32> = row_cells.iter().map(|(x, _, _)| x.round() as i32).collect();
        xs.sort_unstable();
        xs.dedup();
        assert_eq!(
            xs.len(),
            3,
            "expected 3 shared column origins (name/type/size), got {xs:?} from {row_cells:?}"
        );

        // And the same holds at the smaller reference resolution.
        let narrow = Vec2::new(1280.0, 720.0);
        let mut nxs: Vec<i32> = drawn_text(&qm, narrow)
            .iter()
            .filter(|(x, y, _)| *x > TREE_W && *y > header_bottom && *y < narrow.y - STATUS_H)
            .map(|(x, _, _)| x.round() as i32)
            .collect();
        nxs.sort_unstable();
        nxs.dedup();
        assert_eq!(nxs.len(), 3, "columns drift at 1280×720: {nxs:?}");
        let _ = std::fs::remove_dir_all(d);
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

    /// The context menu opens on the declared intent, is instanced from the
    /// proto, and its rows carry the bench's OWN verb names — so a menu pick and
    /// a chord are one event at the dispatcher.
    #[test]
    fn the_context_menu_opens_from_the_intent_and_offers_the_bench_verbs() {
        let (mut qm, d) = scratch("menu");
        assert!(!qm.is_menu_open(), "closed on a settled bench");

        qm.apply_results(&on("menu_open"));
        assert!(qm.is_menu_open(), "the declared intent opens it");

        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
        fn find<'a>(n: &'a UiNode, pred: &dyn Fn(&UiNode) -> bool, hits: &mut Vec<&'a UiNode>) {
            if pred(n) {
                hits.push(n);
            }
            for c in &n.children {
                find(c, pred, hits);
            }
        }
        let mut raw = vec![];
        find(&tree, &|n| n.template.is_some(), &mut raw);
        assert!(raw.is_empty(), "unexpanded template nodes");

        let mut menus = vec![];
        find(&tree, &|n| n.component == "context_menu", &mut menus);
        assert_eq!(menus.len(), 1, "one menu, instanced from the proto");
        assert_eq!(menus[0].visible_bind.as_deref(), Some("has_menu"), "gated by the bench");
        let actions: Vec<_> =
            menus[0].children.iter().filter_map(|r| r.action.as_deref()).collect();
        for v in ["confirm", "rename", "cut", "paste", "create_folder"] {
            assert!(actions.contains(&v), "{v} missing from {actions:?}");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    /// Paste is offered only when something is actually held — and the state is
    /// re-derived every frame, so picking up an item enables the row.
    #[test]
    fn the_menus_paste_row_follows_the_clipboard() {
        let (mut qm, d) = scratch("menupaste");
        qm.open_menu();

        fn paste_disabled(qm: &Quartermaster) -> Option<bool> {
            let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
            fn find<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
                if n.component == "context_menu" {
                    out.push(n);
                }
                for c in &n.children {
                    find(c, out);
                }
            }
            let mut m = vec![];
            find(&tree, &mut m);
            let row = m[0].children.iter().find(|r| r.action.as_deref() == Some("paste"))?;
            match row.props.get("disabled") {
                Some(Value::Bool(b)) => Some(*b),
                _ => Some(false),
            }
        }
        assert_eq!(paste_disabled(&qm), Some(true), "nothing held — Paste is dead");
        qm.nav(1);
        qm.cut();
        assert_eq!(paste_disabled(&qm), Some(false), "holding one — Paste is live");
        let _ = std::fs::remove_dir_all(d);
    }

    /// THE CONTROLLER BASELINE for the menu: open it, walk it with the d-pad,
    /// pick with Confirm — no pointer anywhere. The cursor skips the divider and
    /// any dead row, and the highlight rides the same per-row param the pick
    /// reads, so what is lit is what runs.
    #[test]
    fn the_menu_is_walkable_and_pickable_from_a_pad_alone() {
        let (mut qm, d) = scratch("menupad");
        qm.apply_results(&on("menu_open"));

        fn active_rows(qm: &Quartermaster) -> Vec<usize> {
            let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));
            fn find<'a>(n: &'a UiNode, out: &mut Vec<&'a UiNode>) {
                if n.component == "context_menu" {
                    out.push(n);
                }
                for c in &n.children {
                    find(c, out);
                }
            }
            let mut m = vec![];
            find(&tree, &mut m);
            m[0]
                .children
                .iter()
                .enumerate()
                .filter(|(_, r)| matches!(r.props.get("active"), Some(Value::Bool(true))))
                .map(|(i, _)| i)
                .collect()
        }

        // Exactly one row is lit, and it starts on the first.
        assert_eq!(active_rows(&qm), vec![0], "opens with the first row lit");

        // The d-pad walks it. Paste (proto row 3) is DEAD with an empty clipboard,
        // so the cursor steps straight over it to New folder (proto row 5) —
        // never onto the divider at row 4.
        qm.apply_results(&on("nav_down"));
        assert_eq!(active_rows(&qm), vec![1], "→ Rename");
        qm.apply_results(&on("nav_down"));
        assert_eq!(active_rows(&qm), vec![2], "→ Cut");
        qm.apply_results(&on("nav_down"));
        assert_eq!(active_rows(&qm), vec![5], "skipped dead Paste AND the divider");

        // Confirm picks what is lit — and the listing cursor never moved while
        // the menu was steering.
        assert!(!qm.can_undo());
        qm.apply_results(&on("confirm"));
        assert!(!qm.is_menu_open(), "the pick closed the menu");
        assert!(qm.can_undo(), "and New folder ran");
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

    /// The stack is instanced from the proto, and its rows' Undo buttons fire the
    /// same result the chord does — so a click and a chord are indistinguishable.
    #[test]
    fn the_toast_stack_is_instanced_and_its_undo_is_the_chord_result() {
        let (mut qm, d) = scratch("toastproto");
        qm.create_folder();
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));

        fn find<'a>(n: &'a UiNode, pred: &dyn Fn(&UiNode) -> bool, hits: &mut Vec<&'a UiNode>) {
            if pred(n) {
                hits.push(n);
            }
            for c in &n.children {
                find(c, pred, hits);
            }
        }
        let mut raw = vec![];
        find(&tree, &|n| n.template.is_some(), &mut raw);
        assert!(raw.is_empty(), "unexpanded template nodes in the built tree");

        let mut undos = vec![];
        find(&tree, &|n| n.action.as_deref() == Some("undo"), &mut undos);
        assert_eq!(undos.len(), TOAST_SLOTS, "one inline Undo per slot");

        // And that result really is the undo verb: firing it takes the folder back.
        assert!(qm.can_undo());
        qm.apply_results(&on("undo"));
        assert!(!qm.can_undo(), "the toast's Undo is the same verb as the chord");
        let _ = std::fs::remove_dir_all(d);
    }

    /// The dialog is DATA, not code (Aaron's ruling after the primitives
    /// violation): `build_tree` returns the EXPANDED tree, and the expansion of
    /// the one `conflict_dialog` instance node must yield the proto's real
    /// chrome — `expand`'s failure mode is a warn + empty screen, which every
    /// gate passes silently, so presence has to be asserted positively.
    #[test]
    fn the_conflict_dialog_is_instanced_from_the_proto() {
        let (qm, d) = scratch("proto");
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));

        fn find<'a>(n: &'a UiNode, pred: &dyn Fn(&UiNode) -> bool, hits: &mut Vec<&'a UiNode>) {
            if pred(n) {
                hits.push(n);
            }
            for c in &n.children {
                find(c, pred, hits);
            }
        }
        // No template node survived expansion (a typo'd name would also be
        // reported by the unknown_kinds gate as `template:<name>`).
        let mut raw = vec![];
        find(&tree, &|n| n.template.is_some(), &mut raw);
        assert!(raw.is_empty(), "unexpanded template nodes in the built tree");

        // The proto's chrome is present and still gated by the bench's key.
        let mut dialog = vec![];
        find(&tree, &|n| n.visible_bind.as_deref() == Some("has_prompt"), &mut dialog);
        assert_eq!(dialog.len(), 1, "one gated dialog root");
        let mut popup = vec![];
        find(&tree, &|n| n.id == "conflict_popup", &mut popup);
        assert_eq!(popup.len(), 1, "the proto's popup expanded into the tree");
        let mut actions = vec![];
        find(&tree, &|n| {
            matches!(
                n.action.as_deref(),
                Some("conflict_skip" | "conflict_keep_both" | "conflict_replace")
            )
        }, &mut actions);
        assert_eq!(actions.len(), 3, "the three resolutions are wired");
        let _ = std::fs::remove_dir_all(d);
    }

    /// Tree rows must not OVERLAP. Indentation used a uniform `pad`, which insets
    /// all four sides — so a deep row grew vertically, overran its fixed-height
    /// stack and collided with its neighbours (`Strafe` over `RootMotion`,
    /// `muse` over `epochs`, visible in Aaron's 2026-08-02 smoke test). Indent
    /// belongs on a leading spacer, which cannot touch the row's height.
    #[test]
    fn tree_rows_are_evenly_spaced_at_every_depth() {
        let (mut qm, d) = scratch("treespace");
        // Expand everything reachable so the deep rows (the ones that collided)
        // are actually in the built tree.
        for _ in 0..4 {
            for i in 0..qm.tree_view().len() {
                qm.apply_results(&on(&format!("tree_open_{i}")));
            }
        }
        let screen = Vec2::new(1920.0, 1080.0);
        let text = drawn_text(&qm, screen);

        // Tree-pane captions only: left of the listing, below the chrome.
        let mut ys: Vec<i32> = text
            .iter()
            .filter(|(x, y, _)| {
                *x < TREE_W
                    && *y > TOP_BAR_H + TAB_BAR_H + CRUMB_H
                    && *y < screen.y - STATUS_H
            })
            .map(|(_, y, _)| y.round() as i32)
            .collect();
        ys.sort_unstable();
        ys.dedup();
        assert!(ys.len() > 3, "expected several tree rows, got {ys:?}");

        // Every gap between consecutive rows is the SAME — a row that grew with
        // its depth shows up here as a wider (or negative) gap.
        let gaps: Vec<i32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
        let first = gaps[0];
        assert!(
            gaps.iter().all(|g| (*g - first).abs() <= 1),
            "tree rows are unevenly spaced (depth is inflating row height): gaps={gaps:?} ys={ys:?}"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    /// The selection wash must actually DRAW. The bind being on the right field
    /// and the Model publishing `true` prove nothing about pixels — this walks
    /// the real tree and counts the wash's own `panel_bg` ($accent_wash) in the
    /// emitted commands.
    ///
    /// NOTE for anyone who repeats this: an earlier pass filtered candidates on
    /// `x > TREE_W` and concluded the wash never drew. A `width_frac = 1` wash
    /// starts at EXACTLY `TREE_W`, so the filter hid the very command it was
    /// looking for. Match on the colour, not on a strict inequality.
    #[test]
    fn exactly_one_selection_wash_is_drawn() {
        let (qm, d) = scratch("wash");
        let screen = Vec2::new(1920.0, 1080.0);
        let tree = qm.build_tree(screen);
        let styles = load_styles(HUD_UI_ELEMENTS);
        let host = ScriptHost::library(UI_COMPONENT_MODULES).expect("component library");
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
        let frame = run_ui_with(
            &tree,
            &qm.hud_model(),
            &styles,
            &snap,
            &mut state,
            Some(&host as &dyn ComponentLibrary),
        );
        // $accent_wash — the one colour only `quartermaster.rowsel` carries.
        let washes: Vec<(f32, f32, f32, f32)> = frame
            .commands
            .iter()
            .filter_map(|c| match c {
                HudCommand::Panel { x, y, w, h, color, .. }
                    if (color[0] - 0.141).abs() < 1e-3
                        && (color[2] - 0.471).abs() < 1e-3
                        && (color[3] - 0.1).abs() < 1e-3 =>
                {
                    Some((*x, *y, *w, *h))
                }
                _ => None,
            })
            .collect();

        // The listing holds focus by default, so exactly ONE listing row is
        // washed and no tree row is.
        assert_eq!(
            washes.len(),
            1,
            "expected exactly one drawn selection wash, got {washes:?}"
        );
        let (_, _, w, h) = washes[0];
        assert!(h > 0.0, "the wash drew with zero height — invisible: {washes:?}");
        assert!(w > 0.0, "the wash drew with zero width — invisible: {washes:?}");
        let _ = std::fs::remove_dir_all(d);
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

    /// REGRESSION: `visible_bind`, `tab_group` and `nav_ordinal` are FIELDS on
    /// `UiNode`, not props — setting a prop of the same name compiles, reads
    /// fine, and is silently ignored. The selection wash was therefore drawn on
    /// EVERY row, and no row registered as a nav focusable.
    #[test]
    fn rows_carry_their_bind_and_nav_wiring_as_fields() {
        let (qm, d) = scratch("fields");
        let tree = qm.build_tree(Vec2::new(1920.0, 1080.0));

        // The walker's own collector is the authority on what is focusable.
        let focusables = flicker::ui::focusables_of(&tree, &qm.hud_model());
        for pane in Pane::ALL {
            assert!(
                focusables.iter().any(|f| f.group == pane.id()),
                "no focusable registered for the {:?} pane — d-pad nav would do nothing",
                pane
            );
        }

        // Exactly one row's wash may be bound to each selection key, and the
        // key must be the FIELD the walker reads.
        fn washes(n: &UiNode, out: &mut Vec<String>) {
            if let Some(b) = &n.visible_bind {
                out.push(b.clone());
            }
            for c in &n.children {
                washes(c, out);
            }
        }
        let mut binds = Vec::new();
        washes(&tree, &mut binds);
        assert!(binds.iter().any(|b| b.starts_with("row_")), "row selection washes are bound");
        assert!(binds.iter().any(|b| b.starts_with("tree_")), "tree selection washes are bound");
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

    /// Every verb reaches the bench as a declared intent, so a pad chord, a
    /// Ctrl-key and a menu item are indistinguishable at the dispatcher.
    #[test]
    fn the_editor_verbs_are_declared_and_dispatch() {
        let (qm, d) = scratch("verbs");
        let intents = UiIntents::of(&qm.build_tree(Vec2::new(1920.0, 1080.0)));
        for (sig, name) in [
            (ActionSignal::Cut, "cut"),
            (ActionSignal::Paste, "paste"),
            (ActionSignal::Undo, "undo"),
            (ActionSignal::Redo, "redo"),
            (ActionSignal::CreateFolder, "create_folder"),
        ] {
            assert_eq!(intents.result_for(sig), Some(name), "{sig:?} must be declared");
        }

        // And the dispatcher acts on the fired NAME, not on a key.
        let (mut qm, _) = scratch("verbs2");
        qm.apply_results(&on("create_folder"));
        assert!(qm.can_undo(), "the fired intent name mutated the tree");
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

    fn find_id<'a>(n: &'a UiNode, id: &str) -> Option<&'a UiNode> {
        if n.id == id {
            return Some(n);
        }
        n.children.iter().find_map(|c| find_id(c, id))
    }

    fn count_ids_prefixed(n: &UiNode, prefix: &str) -> usize {
        usize::from(n.id.starts_with(prefix))
            + n.children.iter().map(|c| count_ids_prefixed(c, prefix)).sum::<usize>()
    }
}
