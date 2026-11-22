//! Element ids, and the bit sets selections are made of.
//!
//! Ids are `u32` slot indices, allocated by appending and never reused: a
//! deleted element leaves a dead slot until export. So applying the same
//! `MeshOp`s to the same mesh gives the same ids.

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl $name {
            /// The slot index.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

define_id!(#[doc = "A vertex."] VertId);
define_id!(#[doc = "An edge: two vertices and the faces round it."] EdgeId);
define_id!(#[doc = "A face corner: one step of a face's cycle."] LoopId);
define_id!(#[doc = "A face."] FaceId);

/// A growable set of slot indices.
#[derive(Clone, Debug, Default)]
pub struct BitSet {
    words: Vec<u64>,
}

impl BitSet {
    pub fn contains(&self, i: usize) -> bool {
        self.words.get(i / 64).is_some_and(|w| w & (1u64 << (i % 64)) != 0)
    }

    pub fn insert(&mut self, i: usize) {
        let word = i / 64;
        if word >= self.words.len() {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1u64 << (i % 64);
    }

    pub fn remove(&mut self, i: usize) {
        if let Some(w) = self.words.get_mut(i / 64) {
            *w &= !(1u64 << (i % 64));
        }
    }

    pub fn set(&mut self, i: usize, on: bool) {
        if on { self.insert(i) } else { self.remove(i) }
    }

    pub fn clear(&mut self) {
        self.words.clear();
    }

    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    /// The members, ascending.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(wi, &w)| {
            (0..64).filter(move |b| w & (1u64 << b) != 0).map(move |b| wi * 64 + b)
        })
    }
}

impl PartialEq for BitSet {
    /// Equal members; trailing empty words do not count.
    fn eq(&self, other: &Self) -> bool {
        let n = self.words.len().max(other.words.len());
        (0..n).all(|i| self.words.get(i).copied().unwrap_or(0) == other.words.get(i).copied().unwrap_or(0))
    }
}
