//! **Undo / redo** — the editor spine's command history.
//!
//! The ratified spine says every editor is DOCUMENT + COMMAND HISTORY +
//! WORKFLOW + BENCHES. [`workflow`](crate::workflow) is the ordinal half; this
//! is the reversible half, and it is deliberately the *smallest* thing that
//! satisfies the contract:
//!
//! > Every mutation pushes exactly ONE undo entry, even for a 40-item batch.
//!
//! **A batch is one command, not many.** The history never groups, coalesces or
//! transacts — a caller that mutates forty files builds ONE command holding
//! forty operations, and the history sees a single entry. That keeps the
//! "one mutation, one Ctrl+Z" promise a property of the command, where the
//! caller can actually guarantee it, instead of a heuristic here.
//!
//! # What lives where
//!
//! This module is domain-free: it knows how to stack, cap and re-run things
//! that can [`apply`](Command::apply) and [`revert`](Command::revert). The
//! commands themselves live with the data they mutate — file moves in
//! `flicker_content`, voxel edits in the cluster tools — so this crate never
//! grows a dependency on a content or sim crate. A bench supplies the ten-line
//! `impl Command` that bridges the two.
//!
//! # Failure
//!
//! A revert that fails leaves the world in a state neither side asked for, so
//! the failed command is **kept on the undo stack** rather than silently moved
//! to the redo stack: the user can see it, and a retry is possible. The error
//! is reported, never swallowed.

/// Something a bench did that it can take back.
///
/// `label` is display copy for the toast and the history UI. It is resolved
/// through the stringtable like any other user-visible string — return a
/// `$token`, not English.
pub trait Command {
    /// Do the thing. Called by [`CommandHistory::apply`]; a command pushed with
    /// [`push_applied`](CommandHistory::push_applied) has already had this run.
    fn apply(&mut self) -> Result<(), String>;

    /// Undo exactly what [`apply`](Self::apply) did. Must restore the prior
    /// state well enough that a following `apply` is safe — that round trip is
    /// what the history assumes.
    fn revert(&mut self) -> Result<(), String>;

    /// A `$token` naming this command for the toast / history row.
    fn label(&self) -> &str;
}

/// How many commands a session remembers. The design asks for ≥ 50; the extra
/// headroom costs nothing because a command holds paths, not content.
pub const DEFAULT_DEPTH: usize = 64;

/// A session-scoped undo/redo stack over `C`.
///
/// Not persisted: history dies with the bench, by design — a resumed session
/// cannot honestly promise that the files a stale command references are still
/// where it left them.
#[derive(Debug)]
pub struct CommandHistory<C> {
    /// Applied commands, oldest first; the last is what `undo` takes back.
    done: Vec<C>,
    /// Reverted commands, most-recently-undone last.
    undone: Vec<C>,
    depth: usize,
}

impl<C> Default for CommandHistory<C> {
    fn default() -> Self {
        Self::with_depth(DEFAULT_DEPTH)
    }
}

impl<C> CommandHistory<C> {
    /// A history holding [`DEFAULT_DEPTH`] commands.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A history holding `depth` commands (at least 1 — a zero-depth history
    /// would drop every command it was given, which is never what a caller
    /// means).
    #[must_use]
    pub fn with_depth(depth: usize) -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            depth: depth.max(1),
        }
    }

    /// Is there anything to take back?
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    /// Is there anything to re-do?
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// How many commands are currently undoable.
    #[must_use]
    pub fn len(&self) -> usize {
        self.done.len()
    }

    /// No undoable commands (the freshly-opened state).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.done.is_empty()
    }

    /// Forget everything, both directions.
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }
}

impl<C: Command> CommandHistory<C> {
    /// Record a command whose `apply` the caller has ALREADY run.
    ///
    /// The redo stack is dropped: once a new mutation lands, the branch the
    /// user had undone away from is unreachable, and offering to "redo" onto a
    /// changed world would be a lie.
    pub fn push_applied(&mut self, cmd: C) {
        self.undone.clear();
        self.done.push(cmd);
        // Cap from the OLD end: the deep past is what a user stops caring about.
        while self.done.len() > self.depth {
            self.done.remove(0);
        }
    }

    /// Apply `cmd` and, if it succeeds, record it. A command that fails to
    /// apply is NOT recorded — there is nothing to take back.
    pub fn apply(&mut self, mut cmd: C) -> Result<(), String> {
        cmd.apply()?;
        self.push_applied(cmd);
        Ok(())
    }

    /// Take back the most recent command.
    ///
    /// `None` when there is nothing to undo. `Some(Ok(label))` names what was
    /// undone, for the toast. `Some(Err(..))` means the revert failed and the
    /// command was **kept** on the undo stack.
    pub fn undo(&mut self) -> Option<Result<String, String>> {
        let mut cmd = self.done.pop()?;
        match cmd.revert() {
            Ok(()) => {
                let label = cmd.label().to_string();
                self.undone.push(cmd);
                Some(Ok(label))
            }
            Err(e) => {
                self.done.push(cmd);
                Some(Err(e))
            }
        }
    }

    /// Re-apply the most recently undone command. Same reporting shape as
    /// [`undo`](Self::undo); a failed re-apply is kept on the redo stack.
    pub fn redo(&mut self) -> Option<Result<String, String>> {
        let mut cmd = self.undone.pop()?;
        match cmd.apply() {
            Ok(()) => {
                let label = cmd.label().to_string();
                self.done.push(cmd);
                Some(Ok(label))
            }
            Err(e) => {
                self.undone.push(cmd);
                Some(Err(e))
            }
        }
    }

    /// The `$token` of the command `undo` would take back.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.done.last().map(Command::label)
    }

    /// The `$token` of the command `redo` would re-apply.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.undone.last().map(Command::label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A command over a shared counter, so a test can watch apply/revert order.
    struct Add {
        by: i32,
        log: Rc<RefCell<Vec<i32>>>,
        label: String,
        fail_revert: bool,
        fail_apply: bool,
    }

    impl Add {
        fn new(by: i32, log: &Rc<RefCell<Vec<i32>>>) -> Self {
            Self {
                by,
                log: Rc::clone(log),
                label: format!("$add_{by}"),
                fail_revert: false,
                fail_apply: false,
            }
        }
    }

    impl Command for Add {
        fn apply(&mut self) -> Result<(), String> {
            if self.fail_apply {
                return Err("apply refused".into());
            }
            self.log.borrow_mut().push(self.by);
            Ok(())
        }
        fn revert(&mut self) -> Result<(), String> {
            if self.fail_revert {
                return Err("revert refused".into());
            }
            self.log.borrow_mut().push(-self.by);
            Ok(())
        }
        fn label(&self) -> &str {
            &self.label
        }
    }

    fn sum(log: &Rc<RefCell<Vec<i32>>>) -> i32 {
        log.borrow().iter().sum()
    }

    #[test]
    fn apply_undo_redo_round_trips() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::new();
        h.apply(Add::new(3, &log)).unwrap();
        h.apply(Add::new(5, &log)).unwrap();
        assert_eq!(sum(&log), 8);
        assert_eq!(h.undo_label(), Some("$add_5"));

        assert_eq!(h.undo().unwrap().unwrap(), "$add_5");
        assert_eq!(sum(&log), 3, "the newest command came back first (LIFO)");
        assert_eq!(h.redo().unwrap().unwrap(), "$add_5");
        assert_eq!(sum(&log), 8);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn nothing_to_undo_or_redo_is_none_not_an_error() {
        let mut h: CommandHistory<Add> = CommandHistory::new();
        assert!(h.undo().is_none());
        assert!(h.redo().is_none());
        assert!(h.is_empty());
        assert_eq!(h.undo_label(), None);
    }

    /// THE contract: a 40-item batch is ONE command, so it is ONE Ctrl+Z.
    #[test]
    fn a_batch_is_one_entry_and_one_undo() {
        struct Batch {
            items: Vec<i32>,
            log: Rc<RefCell<Vec<i32>>>,
        }
        impl Command for Batch {
            fn apply(&mut self) -> Result<(), String> {
                self.log.borrow_mut().extend(self.items.iter().copied());
                Ok(())
            }
            fn revert(&mut self) -> Result<(), String> {
                self.log.borrow_mut().extend(self.items.iter().map(|i| -i));
                Ok(())
            }
            fn label(&self) -> &str {
                "$moved_batch"
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::new();
        h.apply(Batch {
            items: (1..=40).collect(),
            log: Rc::clone(&log),
        })
        .unwrap();
        assert_eq!(h.len(), 1, "forty operations, ONE history entry");
        assert_eq!(sum(&log), 820);

        h.undo().unwrap().unwrap();
        assert_eq!(sum(&log), 0, "one undo took back the whole batch");
        assert_eq!(h.len(), 0);
    }

    /// A new mutation makes the undone branch unreachable — offering redo onto
    /// a changed world would be a lie.
    #[test]
    fn a_new_command_invalidates_redo() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::new();
        h.apply(Add::new(1, &log)).unwrap();
        h.undo().unwrap().unwrap();
        assert!(h.can_redo());

        h.apply(Add::new(9, &log)).unwrap();
        assert!(!h.can_redo(), "the undone branch is gone");
        assert!(h.redo().is_none());
    }

    #[test]
    fn depth_caps_from_the_old_end() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::with_depth(3);
        for i in 1..=5 {
            h.apply(Add::new(i, &log)).unwrap();
        }
        assert_eq!(h.len(), 3, "capped");
        // 4 and 5 are the survivors; 1..3 aged out.
        assert_eq!(h.undo_label(), Some("$add_5"));
        h.undo().unwrap().unwrap();
        assert_eq!(h.undo_label(), Some("$add_4"));
        h.undo().unwrap().unwrap();
        assert_eq!(h.undo_label(), Some("$add_3"));
        h.undo().unwrap().unwrap();
        assert!(!h.can_undo());
    }

    #[test]
    fn a_zero_depth_history_still_holds_one_command() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::with_depth(0);
        h.apply(Add::new(1, &log)).unwrap();
        assert_eq!(
            h.len(),
            1,
            "a history that drops everything is never what a caller meant"
        );
    }

    /// A failed revert must not pretend it worked: the command stays undoable.
    #[test]
    fn a_failed_revert_keeps_the_command_undoable() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::new();
        let mut cmd = Add::new(2, &log);
        cmd.fail_revert = true;
        h.apply(cmd).unwrap();

        let err = h.undo().unwrap().unwrap_err();
        assert_eq!(err, "revert refused");
        assert_eq!(
            h.len(),
            1,
            "still on the undo stack — the user can see and retry it"
        );
        assert!(!h.can_redo(), "and it did NOT move to redo");
    }

    /// A command that fails to apply has nothing to take back.
    #[test]
    fn a_failed_apply_is_not_recorded() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut h = CommandHistory::new();
        let mut cmd = Add::new(2, &log);
        cmd.fail_apply = true;
        assert!(h.apply(cmd).is_err());
        assert!(
            h.is_empty(),
            "nothing happened, so there is nothing to undo"
        );
    }
}
