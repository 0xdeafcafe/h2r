//! A snapshot undo stack any tool can use.
//!
//! The usual pattern in an immediate-mode tool: clone the edited state
//! before drawing, draw (widgets mutate it), then hand both to
//! [`UndoStack::track`]. It records the "before" snapshot only when the state
//! changed, and coalesces a run of changes from one widget (typing into a
//! field, dragging a number) into a single step.
//!
//! Snapshots are whole clones, so keep the tracked state small or share big
//! immutable parts with `Arc` (the hopper tool shares its 1.4 MB image that
//! way).

/// Undo/redo of a `T` by snapshots.
#[derive(Debug, Clone)]
pub struct UndoStack<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    limit: usize,
    /// The widget the last recorded change came from: further changes from
    /// it extend that step instead of adding one.
    coalesce: Option<egui::Id>,
}

impl<T> Default for UndoStack<T> {
    fn default() -> Self {
        Self::new(256)
    }
}

impl<T> UndoStack<T> {
    /// A stack keeping at most `limit` undo steps (the oldest drop first).
    pub fn new(limit: usize) -> Self {
        UndoStack {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            coalesce: None,
        }
    }

    /// Records `before` as one undo step (the state just before an edit)
    /// and forgets the redo history.
    pub fn record(&mut self, before: T) {
        self.coalesce = None;
        self.push(before);
    }

    fn push(&mut self, before: T) {
        self.redo.clear();
        self.undo.push(before);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
    }

    /// Records `before` unless the previous step came from the same `key`
    /// (one widget's run of edits is one step).
    pub fn record_coalesced(&mut self, before: T, key: egui::Id) {
        if self.coalesce == Some(key) && !self.undo.is_empty() {
            self.redo.clear();
            return;
        }
        self.push(before);
        self.coalesce = Some(key);
    }

    /// Ends coalescing: the next change is a new step even from the same
    /// widget.
    pub fn break_coalescing(&mut self) {
        self.coalesce = None;
    }

    /// Swaps `current` for the last undo step; false when there is none.
    pub fn undo(&mut self, current: &mut T) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(current, prev));
        self.coalesce = None;
        true
    }

    /// Swaps `current` for the last undone state; false when there is none.
    pub fn redo(&mut self, current: &mut T) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(current, next));
        self.coalesce = None;
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Forgets every step (a newly opened file).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.coalesce = None;
    }
}

impl<T: PartialEq> UndoStack<T> {
    /// Records `before` if `after` differs from it: one step per change,
    /// coalesced per `key` when given (pass the focused widget's id for
    /// text fields and drag values). Returns whether anything changed.
    pub fn track(&mut self, before: T, after: &T, key: Option<egui::Id>) -> bool {
        if before == *after {
            return false;
        }
        match key {
            Some(k) => self.record_coalesced(before, k),
            None => self.record(before),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut s = UndoStack::default();
        let mut v = 1;
        s.record(v);
        v = 2;
        s.record(v);
        v = 3;
        assert!(s.undo(&mut v));
        assert_eq!(v, 2);
        assert!(s.undo(&mut v));
        assert_eq!(v, 1);
        assert!(!s.undo(&mut v));
        assert_eq!(v, 1);
        assert!(s.redo(&mut v));
        assert!(s.redo(&mut v));
        assert_eq!(v, 3);
        assert!(!s.redo(&mut v));
    }

    #[test]
    fn a_new_edit_drops_the_redo_history() {
        let mut s = UndoStack::default();
        let mut v = 1;
        s.record(v);
        v = 2;
        assert!(s.undo(&mut v));
        assert!(s.can_redo());
        s.record(v);
        assert!(!s.can_redo());
    }

    #[test]
    fn edits_from_one_widget_coalesce() {
        let mut s = UndoStack::default();
        let field = egui::Id::unique("title");
        let other = egui::Id::unique("desc");
        let mut v = String::new();
        for c in ["a", "ab", "abc"] {
            let before = v.clone();
            v = c.into();
            assert!(s.track(before, &v, Some(field)));
        }
        let before = v.clone();
        v = "abc!".into();
        s.track(before, &v, Some(other));
        let same = v.clone();
        assert!(!s.track(same, &v, Some(other)), "no change, no step");
        assert!(s.undo(&mut v));
        assert_eq!(v, "abc");
        assert!(s.undo(&mut v));
        assert_eq!(v, "", "the typing run is one step");
        assert!(!s.can_undo());
    }

    #[test]
    fn coalescing_ends_after_undo_or_a_break() {
        let mut s = UndoStack::default();
        let k = egui::Id::unique("k");
        s.track(0, &1, Some(k));
        s.break_coalescing();
        s.track(1, &2, Some(k));
        let mut v = 2;
        assert!(s.undo(&mut v));
        assert_eq!(v, 1);
        s.track(1, &5, Some(k));
        let mut v = 5;
        assert!(s.undo(&mut v));
        assert_eq!(v, 1, "after an undo the same widget starts a new step");
    }

    #[test]
    fn the_limit_drops_the_oldest_steps() {
        let mut s = UndoStack::new(2);
        for n in 0..5 {
            s.record(n);
        }
        let mut v = 5;
        assert!(s.undo(&mut v));
        assert!(s.undo(&mut v));
        assert_eq!(v, 3);
        assert!(!s.undo(&mut v));
    }
}
