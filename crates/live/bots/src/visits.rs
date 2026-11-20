//! Scratch for a spatial query which can encounter the same index in
//! several grid cells. Queries run to completion on the calling thread.
use std::cell::RefCell;

#[derive(Default)]
pub(crate) struct Visits {
    marks: Vec<u32>,
    generation: u32,
}

impl Visits {
    fn begin(&mut self, len: usize) {
        if self.marks.len() < len {
            self.marks.resize(len, 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.marks.fill(0);
            self.generation = 1;
        }
    }

    pub(crate) fn insert(&mut self, index: u32) -> bool {
        let mark = &mut self.marks[index as usize];
        std::mem::replace(mark, self.generation) != self.generation
    }

    /// Leaf queries only: no calling back into another grid query.
    pub(crate) fn query<T>(len: usize, f: impl FnOnce(&mut Self) -> T) -> T {
        thread_local! { static VISITS: RefCell<Visits> = RefCell::default(); }
        VISITS.with_borrow_mut(|seen| {
            seen.begin(len);
            f(seen)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visit_marks_survive_growth_and_generation_wrap() {
        let mut seen = Visits::default();
        seen.begin(2);
        assert!(seen.insert(1));
        assert!(!seen.insert(1));
        seen.generation = u32::MAX;
        seen.begin(3);
        assert!(seen.insert(1));
        assert!(seen.insert(2));
        seen.begin(1);
        seen.begin(3);
        assert!(seen.insert(2));
    }
}
