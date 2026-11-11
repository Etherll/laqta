//! Undo by commands (ADR 009). Only destructive operations keep pixel snapshots, under a byte cap.

use crate::fx::Img;
use crate::model::Annotation;

/// The default cap on snapshot bytes kept for undo.
pub const DEFAULT_SNAPSHOT_CAP: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub base: Img,
    pub annotations: Vec<Annotation>,
}

#[derive(Clone, Debug)]
pub(crate) enum Entry {
    Add {
        index: usize,
        annotation: Annotation,
    },
    Remove {
        index: usize,
        annotation: Annotation,
    },
    Modify {
        before: Annotation,
        after: Annotation,
    },
    Reorder {
        before: Vec<u64>,
        after: Vec<u64>,
    },
    Destructive {
        before: Box<Snapshot>,
        after: Box<Snapshot>,
    },
    /// Several entries made by one command, undone in reverse and redone in order.
    Batch(Vec<Entry>),
}

impl Entry {
    fn bytes(&self) -> usize {
        match self {
            Entry::Destructive { before, after } => before.base.px.len() + after.base.px.len(),
            Entry::Batch(entries) => entries.iter().map(Entry::bytes).sum(),
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct History {
    pub undo: Vec<Entry>,
    pub redo: Vec<Entry>,
    pub cap: usize,
}

impl History {
    pub fn new() -> History {
        History { undo: Vec::new(), redo: Vec::new(), cap: DEFAULT_SNAPSHOT_CAP }
    }

    pub fn push(&mut self, e: Entry) {
        self.redo.clear();
        self.undo.push(e);
        self.enforce_cap();
    }

    /// Drops the oldest entries until the snapshot bytes fit under the cap.
    pub fn enforce_cap(&mut self) {
        let total = |h: &History| h.undo.iter().chain(&h.redo).map(Entry::bytes).sum::<usize>();
        while total(self) > self.cap && !self.undo.is_empty() {
            self.undo.remove(0);
        }
    }

    pub fn snapshot_bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(Entry::bytes).sum()
    }
}
