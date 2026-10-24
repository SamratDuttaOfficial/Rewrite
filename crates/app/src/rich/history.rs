//! Undo for formatted documents. GTK's own undo forgets formatting and embedded objects, so in this mode
//! every change to the buffer is recorded here: content put in (Put), taken out (Cut), a document tag's
//! ranges changed (Tag), or an object edited in place (Change).
//!
//! The changes made during one user action (a key press, a paste, one app.one_step) form one step.
//! Typing and erasing one character at a time join the step before, until a word ends, the kind of edit
//! changes or there is a pause of two seconds.

use rewrite_core::flat::Obj;
use std::time::{Duration, Instant};

/// A piece of the document taken out of the buffer, to be put back later.
#[derive(Clone, Debug)]
pub enum Item {
    /// Text and its document tags.
    Text(String, Vec<String>),
    /// An embedded object (id, object, its tags).
    Obj(String, Obj, Vec<String>),
    /// A list marker and its tags.
    Marker(Vec<String>),
}

pub type Frag = Vec<Item>;

#[derive(Clone, Debug)]
pub enum Op {
    /// Content put in at i1 (it ends at i2).
    Put { i1: usize, i2: usize, frag: Frag },
    /// Content taken out between i1 and i2.
    Cut { i1: usize, i2: usize, frag: Frag },
    /// A tag's ranges between i1 and i2 before and after a change.
    Tag { tag: String, i1: usize, i2: usize, before: Vec<(usize, usize)>, after: Vec<(usize, usize)> },
    /// An object (a picture, chart, table ...) edited in place.
    Change { oid: String, before: Obj, after: Obj },
}

struct Action {
    ops: Vec<Op>,
    /// type | type:sp | erase, when the action is one such edit
    kind: Option<&'static str>,
    primaries: u32,
}

#[derive(Default)]
pub struct History {
    pub done: Vec<Vec<Op>>,
    pub undone: Vec<Vec<Op>>,
    cur: Option<Action>,
    /// What the latest step holds (for joining typing), None after a separator.
    last_kind: Option<&'static str>,
    at: Option<Instant>,
}

const MAX_STEPS: usize = 400;

impl History {
    pub fn reset(&mut self) {
        *self = History::default();
    }

    /// An edit of this kind starts (at the outermost level of a change).
    pub fn begin(&mut self, kind: Option<&'static str>) {
        let a = self.cur.get_or_insert(Action { ops: vec![], kind, primaries: 0 });
        a.primaries += 1;
        if a.primaries > 1 {
            a.kind = None;
        }
    }

    pub fn record(&mut self, op: Op) {
        self.cur.get_or_insert(Action { ops: vec![], kind: None, primaries: 0 }).ops.push(op);
        self.undone.clear();
    }

    /// The user action is over: its changes become a step of their own, or join the typing before.
    pub fn finish(&mut self) {
        let Some(a) = self.cur.take() else { return };
        if a.ops.is_empty() {
            return;
        }
        let now = Instant::now();
        let same = match (self.last_kind, a.kind) {
            (Some(prev), Some(k)) => k == prev || (prev == "type" && k == "type:sp"),
            _ => false,
        };
        let recent = self.at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(2));
        match self.done.last_mut() {
            Some(last) if same && recent => last.extend(a.ops),
            _ => {
                self.done.push(a.ops);
                if self.done.len() > MAX_STEPS {
                    self.done.remove(0);
                }
            }
        }
        self.last_kind = a.kind;
        self.at = Some(now);
    }

    /// Nothing that follows joins the step before.
    pub fn separator(&mut self) {
        self.finish();
        self.last_kind = None;
    }

    pub fn take_undo(&mut self) -> Option<Vec<Op>> {
        self.separator();
        while self.done.last().is_some_and(|g| g.is_empty()) {
            self.done.pop();
        }
        self.done.pop()
    }

    pub fn take_redo(&mut self) -> Option<Vec<Op>> {
        self.separator();
        self.undone.pop()
    }
}
