//! Frame-buffer copies shared between draws (docs/graphics/d3d.md, "Frame
//! buffer as a texture").
//!
//! A draw that samples a texture over a frame buffer reads a copy of it
//! ([`crate::Backend::snapshot_surface`]). Each colour surface the device
//! writes has a colour-write generation, bumped by every draw, clear, copy,
//! upload or pass the device issues into it. A copy is taken at the
//! source's generation and shared by every later draw that samples the
//! same surface (with the same alpha rule) while the generation is
//! unchanged, so N sampling draws with no write to the source in between
//! cost one copy, and a write in between makes the next one fresh.
//!
//! At most [`KEPT`] copies are held at once (least recently used goes
//! first), a write to a surface drops its copies, a destroyed surface
//! forgets its generation, and a present drops every copy, so a long
//! session holds no more than [`KEPT`] whatever it draws.

/// Most frame-buffer copies held at once: the frame buffers (up to three)
/// and a colour surface over the depth buffer, each with and without
/// forced alpha, fit.
pub(super) const KEPT: usize = 8;

struct Entry<S, T> {
    id: u64,
    surface: S,
    opaque: bool,
    generation: u64,
    used: u64,
    texture: T,
}

/// The shared copies and the colour-write generation of each surface.
pub(super) struct FrameSnapshots<S, T> {
    generations: Vec<(S, u64)>,
    /// Source of generations, ids and use stamps: never repeats.
    counter: u64,
    entries: Vec<Entry<S, T>>,
}

impl<S, T> Default for FrameSnapshots<S, T> {
    fn default() -> Self {
        Self {
            generations: Vec::new(),
            counter: 0,
            entries: Vec::new(),
        }
    }
}

impl<S: Copy + Eq, T> FrameSnapshots<S, T> {
    fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// `surface`'s colour-write generation: 0 until it is first written.
    pub(super) fn generation(&self, surface: S) -> u64 {
        self.generations
            .iter()
            .find(|(s, _)| *s == surface)
            .map_or(0, |&(_, g)| g)
    }

    /// The device wrote `surface`: its generation moves on and its copies,
    /// now stale, are dropped.
    pub(super) fn written(&mut self, surface: S) {
        let g = self.next();
        match self.generations.iter_mut().find(|(s, _)| *s == surface) {
            Some(slot) => slot.1 = g,
            None => self.generations.push((surface, g)),
        }
        self.entries.retain(|e| e.surface != surface);
    }

    /// `surface` was destroyed: a later surface with the same handle starts
    /// with no generation and no copies.
    pub(super) fn forget(&mut self, surface: S) {
        self.generations.retain(|(s, _)| *s != surface);
        self.entries.retain(|e| e.surface != surface);
    }

    /// Drops every copy (a present, a reset, a snapshot load). Generations
    /// go too: with no copies left nothing compares against them.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.generations.clear();
    }

    /// The copy of `surface` taken at its current generation, if one is
    /// held: its id for [`Self::texture`].
    pub(super) fn find(&mut self, surface: S, opaque: bool) -> Option<u64> {
        let generation = self.generation(surface);
        let stamp = self.next();
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.surface == surface && e.opaque == opaque && e.generation == generation)?;
        e.used = stamp;
        Some(e.id)
    }

    /// Holds `texture`, a copy of `surface` as it is now, and returns its
    /// id. Replaces an older copy of the same surface and alpha rule, and
    /// drops the least recently used one when [`KEPT`] are held.
    pub(super) fn insert(&mut self, surface: S, opaque: bool, texture: T) -> u64 {
        self.entries
            .retain(|e| !(e.surface == surface && e.opaque == opaque));
        if self.entries.len() >= KEPT
            && let Some(i) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i)
        {
            self.entries.swap_remove(i);
        }
        let id = self.next();
        let generation = self.generation(surface);
        self.entries.push(Entry {
            id,
            surface,
            opaque,
            generation,
            used: id,
            texture,
        });
        id
    }

    /// The copy with id `id`, while it is held.
    pub(super) fn texture(&self, id: u64) -> Option<&T> {
        self.entries.iter().find(|e| e.id == id).map(|e| &e.texture)
    }

    /// Copies held now.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_shared_until_its_surface_is_written() {
        let mut f = FrameSnapshots::<u32, &str>::default();
        assert_eq!(f.find(1, false), None);
        let a = f.insert(1, false, "a");
        assert_eq!(f.find(1, false), Some(a));
        // Another alpha rule, another surface: their own copies.
        assert_eq!(f.find(1, true), None);
        assert_eq!(f.find(2, false), None);
        // A write to another surface keeps it.
        f.written(2);
        assert_eq!(f.find(1, false), Some(a));
        f.written(1);
        assert_eq!(f.find(1, false), None);
        assert_eq!(f.len(), 0);
        let b = f.insert(1, false, "b");
        assert_ne!(a, b);
        assert_eq!(f.texture(b), Some(&"b"));
        assert_eq!(f.texture(a), None);
    }

    #[test]
    fn held_copies_stay_bounded() {
        let mut f = FrameSnapshots::<u32, u32>::default();
        for i in 0..1000u32 {
            let id = f.insert(i % 13, i % 2 == 0, i);
            assert!(f.len() <= KEPT);
            assert_eq!(f.texture(id), Some(&i));
        }
        f.forget(3);
        assert!(f.entries.iter().all(|e| e.surface != 3));
        f.clear();
        assert_eq!(f.len(), 0);
        assert!(f.generations.is_empty());
    }
}
