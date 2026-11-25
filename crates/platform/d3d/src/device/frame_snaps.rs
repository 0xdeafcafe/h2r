//! Frame-buffer copies: a fixed pool, shared between draws (docs/graphics/
//! d3d.md, "Frame buffers as textures").
//!
//! A draw that samples a texture over a frame buffer reads a copy of it.
//! The copies are a pool of textures per surface size and format, at most
//! [`SLOTS`] each and [`MAX_SLOTS`] in all. A slot's texture is made once
//! ([`crate::Backend::snapshot_surface`]) and refilled by a GPU copy
//! ([`crate::Backend::snapshot_surface_into`]) for later draws, so the
//! number of draws that sample the frame never raises the textures held.
//! Each copy and the draw that reads it are recorded in order, so a slot
//! refilled for a later draw is still read with its old contents by the
//! draws recorded before the refill.
//!
//! Each colour surface the device writes has a colour-write generation,
//! bumped by every draw, clear, copy, upload or pass the device issues
//! into it. A slot remembers the surface, alpha rule and generation it
//! holds, and every later draw that samples the same surface (with the
//! same alpha rule) while the generation is unchanged shares it with no
//! copy at all; a write in between makes the next sample a fresh copy.
//!
//! The slots a draw binds are pinned for that draw ([`FrameSnapshots::
//! begin_draw`]): another stage of the same draw never refills them. A
//! write to a surface or a present ends what the slots hold, and keeps the
//! textures; a destroyed surface forgets its generation.

/// Copies held for one surface size and format: the current frame, the
/// one last presented and a third, with or without forced alpha. A draw
/// that binds more distinct copies of one size than this (up to its four
/// stages) adds a slot rather than refill one it binds.
pub const SLOTS: usize = 3;

/// Copy textures held at most, over every size and format: a new size past
/// this replaces the least recently used slot of another.
pub const MAX_SLOTS: usize = 8;

/// What a slot holds: `surface` at `generation`, alpha one with `opaque`.
#[derive(Clone, Copy)]
struct Content<S> {
    surface: S,
    opaque: bool,
    generation: u64,
}

struct Slot<S, T, K> {
    id: u64,
    key: K,
    texture: T,
    content: Option<Content<S>>,
    used: u64,
    /// The draw that last bound it ([`FrameSnapshots::begin_draw`]).
    draw: u64,
}

/// The pool of copies and the colour-write generation of each surface.
/// `K` is a slot's size and format.
pub(super) struct FrameSnapshots<S, T, K> {
    generations: Vec<(S, u64)>,
    /// Source of generations, ids, draws and use stamps: never repeats.
    counter: u64,
    draw: u64,
    slots: Vec<Slot<S, T, K>>,
}

impl<S, T, K> Default for FrameSnapshots<S, T, K> {
    fn default() -> Self {
        Self {
            generations: Vec::new(),
            counter: 0,
            draw: 0,
            slots: Vec::new(),
        }
    }
}

impl<S: Copy + Eq, T, K: Copy + Eq> FrameSnapshots<S, T, K> {
    fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn at(&self, id: u64) -> Option<usize> {
        self.slots.iter().position(|s| s.id == id)
    }

    /// A draw starts binding its textures: the slots it binds from here on
    /// are pinned until the next.
    pub(super) fn begin_draw(&mut self) {
        self.draw = self.next();
    }

    /// `surface`'s colour-write generation: 0 until it is first written.
    pub(super) fn generation(&self, surface: S) -> u64 {
        self.generations
            .iter()
            .find(|(s, _)| *s == surface)
            .map_or(0, |&(_, g)| g)
    }

    /// The device wrote `surface`: its generation moves on and the slots
    /// holding it are stale.
    pub(super) fn written(&mut self, surface: S) {
        let g = self.next();
        match self.generations.iter_mut().find(|(s, _)| *s == surface) {
            Some(slot) => slot.1 = g,
            None => self.generations.push((surface, g)),
        }
        self.drop_contents(|c| c.surface == surface);
    }

    /// `surface` was destroyed: a later surface with the same handle starts
    /// with no generation and nothing shared.
    pub(super) fn forget(&mut self, surface: S) {
        self.generations.retain(|(s, _)| *s != surface);
        self.drop_contents(|c| c.surface == surface);
    }

    /// A present: nothing the slots hold is shared past it (a frame owner
    /// replays each frame's samples with that frame). The textures stay
    /// for the next frame's copies. Generations go too: with nothing held
    /// nothing compares against them.
    pub(super) fn present(&mut self) {
        self.drop_contents(|_| true);
        self.generations.clear();
    }

    /// Drops every slot and its texture (a reset, a snapshot load, the
    /// device released).
    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.generations.clear();
    }

    fn drop_contents(&mut self, which: impl Fn(&Content<S>) -> bool) {
        for s in &mut self.slots {
            if s.content.as_ref().is_some_and(&which) {
                s.content = None;
            }
        }
    }

    /// The slot holding `surface` at its current generation, if one does:
    /// its id for [`Self::texture`], pinned for this draw.
    pub(super) fn find(&mut self, surface: S, opaque: bool) -> Option<u64> {
        let generation = self.generation(surface);
        let (stamp, draw) = (self.next(), self.draw);
        let s = self.slots.iter_mut().find(|s| {
            s.content.is_some_and(|c| {
                c.surface == surface && c.opaque == opaque && c.generation == generation
            })
        })?;
        s.used = stamp;
        s.draw = draw;
        Some(s.id)
    }

    /// The least recently used slot this draw has not bound, of size `key`
    /// alone or of any, and holding nothing with `empty`.
    fn lru(&self, key: Option<K>, empty: bool) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.draw != self.draw
                    && key.is_none_or(|k| s.key == k)
                    && (!empty || s.content.is_none())
            })
            .min_by_key(|(_, s)| s.used)
            .map(|(i, _)| i)
    }

    /// A slot of size `key` to refill for a new copy: its id, pinned for
    /// this draw and holding nothing until [`Self::filled`]. A slot that
    /// holds nothing any more (its surface was written, or a present came)
    /// goes first, then, with [`SLOTS`] of this size held, the least
    /// recently used. `None`: make a new texture and [`Self::insert`] it
    /// (room for it is made here, by dropping the least recently used slot
    /// of another size when [`MAX_SLOTS`] are held).
    ///
    /// A draw that samples the frame it writes therefore refills the one
    /// slot the draw before it read: the copy is recorded after that draw,
    /// so it read the old contents.
    pub(super) fn slot_for(&mut self, key: K) -> Option<u64> {
        let own = self.slots.iter().filter(|s| s.key == key).count();
        let pick = self.lru(Some(key), true);
        let pick = pick.or_else(|| if own >= SLOTS { self.lru(Some(key), false) } else { None });
        let pick = pick.or_else(|| {
            (self.slots.len() >= MAX_SLOTS)
                .then(|| self.lru(None, false))
                .flatten()
        });
        let i = pick?;
        if self.slots[i].key != key {
            self.slots.swap_remove(i);
            return None;
        }
        let (stamp, draw) = (self.next(), self.draw);
        let s = &mut self.slots[i];
        s.content = None;
        s.used = stamp;
        s.draw = draw;
        Some(s.id)
    }

    /// Slot `id` now holds `surface` as it is (alpha one with `opaque`).
    pub(super) fn filled(&mut self, id: u64, surface: S, opaque: bool) {
        let generation = self.generation(surface);
        if let Some(i) = self.at(id) {
            self.slots[i].content = Some(Content {
                surface,
                opaque,
                generation,
            });
        }
    }

    /// The backend could not refill slot `id`: `texture`, a new copy of
    /// `surface`, takes its place.
    pub(super) fn replace(&mut self, id: u64, surface: S, opaque: bool, texture: T) {
        if let Some(i) = self.at(id) {
            self.slots[i].texture = texture;
        }
        self.filled(id, surface, opaque);
    }

    /// A new slot of size `key` holding `texture`, a copy of `surface` as
    /// it is now, pinned for this draw: its id.
    pub(super) fn insert(&mut self, key: K, surface: S, opaque: bool, texture: T) -> u64 {
        let id = self.next();
        let generation = self.generation(surface);
        self.slots.push(Slot {
            id,
            key,
            texture,
            content: Some(Content {
                surface,
                opaque,
                generation,
            }),
            used: id,
            draw: self.draw,
        });
        id
    }

    /// Slot `id`'s texture.
    pub(super) fn texture(&self, id: u64) -> Option<&T> {
        self.slots.iter().find(|s| s.id == id).map(|s| &s.texture)
    }

    /// Slot `id`'s texture, to refill.
    pub(super) fn texture_mut(&mut self, id: u64) -> Option<&mut T> {
        self.slots.iter_mut().find(|s| s.id == id).map(|s| &mut s.texture)
    }

    /// Copy textures held now.
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Pool = FrameSnapshots<u32, u32, u8>;

    /// What a sampling draw does: the shared slot, else a refilled one,
    /// else a new texture (numbered from 100 by `made`).
    fn sample(f: &mut Pool, key: u8, surface: u32, opaque: bool, made: &mut u32) -> u64 {
        if let Some(id) = f.find(surface, opaque) {
            return id;
        }
        if let Some(id) = f.slot_for(key) {
            f.filled(id, surface, opaque);
            return id;
        }
        *made += 1;
        f.insert(key, surface, opaque, 100 + *made)
    }

    #[test]
    fn a_copy_is_shared_until_its_surface_is_written() {
        let mut f = Pool::default();
        let mut made = 0;
        f.begin_draw();
        assert_eq!(f.find(1, false), None);
        let a = sample(&mut f, 0, 1, false, &mut made);
        f.begin_draw();
        assert_eq!(f.find(1, false), Some(a));
        // Another alpha rule, another surface: their own copies.
        assert_eq!(f.find(1, true), None);
        assert_eq!(f.find(2, false), None);
        // A write to another surface keeps it.
        f.written(2);
        assert_eq!(f.find(1, false), Some(a));
        f.written(1);
        assert_eq!(f.find(1, false), None);
        // The texture stays: only what it holds ended.
        assert_eq!(f.len(), 1);
        f.begin_draw();
        let b = sample(&mut f, 0, 1, false, &mut made);
        assert_eq!(a, b, "the slot that holds nothing any more is refilled");
        assert_eq!(made, 1);
        assert_eq!(f.find(1, false), Some(b));
    }

    /// Every sample after a write is a fresh copy, into the slot the write
    /// emptied: one texture for any number of them.
    #[test]
    fn copies_between_writes_refill_a_fixed_pool() {
        let mut f = Pool::default();
        let mut made = 0;
        for _ in 0..500 {
            f.begin_draw();
            sample(&mut f, 0, 1, false, &mut made);
            // The draw writes the frame it sampled.
            f.written(1);
            assert!(f.len() <= SLOTS);
        }
        assert_eq!(made, 1);
    }

    /// More distinct copies in use than [`SLOTS`]: the least recently used
    /// is refilled, and the textures made stop at the pool size.
    #[test]
    fn distinct_copies_past_the_pool_refill_the_least_recently_used() {
        let mut f = Pool::default();
        let mut made = 0;
        let mut ids = Vec::new();
        for i in 0..500u32 {
            f.begin_draw();
            ids.push(sample(&mut f, 0, i % 4, false, &mut made));
            assert!(f.len() <= SLOTS);
        }
        assert_eq!(made as usize, SLOTS);
        // A refill never takes the slot the draw before bound.
        assert!(ids.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn a_draw_never_refills_a_slot_it_binds() {
        let mut f = Pool::default();
        let mut made = 0;
        // Fill the pool for size 0.
        for s in 0..SLOTS as u32 {
            f.begin_draw();
            sample(&mut f, 0, s, false, &mut made);
        }
        // One draw binds four distinct copies of size 0.
        f.begin_draw();
        let ids: Vec<_> = (10..14).map(|s| sample(&mut f, 0, s, false, &mut made)).collect();
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 4, "{ids:?}");
        assert_eq!(f.len(), 4);
        for (s, id) in (10..14).zip(&ids) {
            assert_eq!(f.find(s, false), Some(*id));
        }
    }

    #[test]
    fn held_copies_stay_bounded() {
        let mut f = Pool::default();
        let mut made = 0;
        for i in 0..1000u32 {
            f.begin_draw();
            let id = sample(&mut f, (i % 5) as u8, i % 13, i % 2 == 0, &mut made);
            assert!(f.len() <= MAX_SLOTS);
            assert!(f.texture(id).is_some());
            if i % 7 == 0 {
                f.present();
            }
        }
        f.forget(3);
        assert!(f.slots.iter().all(|s| s.content.is_none_or(|c| c.surface != 3)));
        f.present();
        assert!(f.slots.iter().all(|s| s.content.is_none()));
        assert!(f.len() > 0, "a present keeps the textures");
        f.clear();
        assert_eq!(f.len(), 0);
        assert!(f.generations.is_empty());
    }
}
