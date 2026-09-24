use ropey::Rope;

use crate::change::ChangeSet;
use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoMode {
    /// Start a new undo step.
    NewStep,
    /// Add to the last undo step, e.g. while typing in insert mode.
    Merge,
}

struct Revision {
    id: u64,
    changes: ChangeSet,
    inverse: ChangeSet,
    before: Selection,
    after: Selection,
}

/// Linear undo history. Each step is a group of revisions undone together.
#[derive(Default)]
pub(crate) struct History {
    steps: Vec<Vec<Revision>>,
    applied: usize,
    /// Merging into a step is only allowed right after recording into it,
    /// never after an undo or redo.
    can_merge: bool,
    last_id: u64,
}

impl History {
    pub fn record(
        &mut self,
        changes: ChangeSet,
        inverse: ChangeSet,
        before: Selection,
        after: Selection,
        mode: UndoMode,
    ) {
        self.steps.truncate(self.applied);
        self.last_id += 1;
        let revision = Revision {
            id: self.last_id,
            changes,
            inverse,
            before,
            after,
        };
        match self.steps.last_mut() {
            Some(step) if mode == UndoMode::Merge && self.can_merge => step.push(revision),
            _ => {
                self.steps.push(vec![revision]);
                self.applied += 1;
            }
        }
        self.can_merge = true;
    }

    /// Reverts the last applied step. Returns the change sets applied to
    /// `text` in order, and the selection from before the step.
    pub fn undo(&mut self, text: &mut Rope) -> Option<(Vec<ChangeSet>, Selection)> {
        self.applied = self.applied.checked_sub(1)?;
        self.can_merge = false;
        let step = &self.steps[self.applied];
        let changes = step
            .iter()
            .rev()
            .map(|revision| {
                revision.inverse.apply(text);
                revision.inverse.clone()
            })
            .collect();
        Some((changes, step[0].before.clone()))
    }

    /// Reapplies the next undone step. Returns the change sets applied to
    /// `text` in order, and the selection from after the step.
    pub fn redo(&mut self, text: &mut Rope) -> Option<(Vec<ChangeSet>, Selection)> {
        let step = self.steps.get(self.applied)?;
        let changes = step
            .iter()
            .map(|revision| {
                revision.changes.apply(text);
                revision.changes.clone()
            })
            .collect();
        let selection = step[step.len() - 1].after.clone();
        self.applied += 1;
        self.can_merge = false;
        Some((changes, selection))
    }

    /// Identifies the current state of the text, to tell whether it has
    /// changed since it was saved.
    pub fn state(&self) -> u64 {
        match self.applied {
            0 => 0,
            n => self.steps[n - 1].last().map_or(0, |revision| revision.id),
        }
    }
}
