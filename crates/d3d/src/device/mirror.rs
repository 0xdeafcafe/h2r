//! Overlapping render-to-texture levels on the GPU: the alias mirror.
//!
//! Two level headers over one allocation (Halo's pool of render targets:
//! 160x120 and 320x240 colour levels and the 512x512 `LIN_D24S8` shadow
//! map) used to move pixels through guest memory: binding or sampling one
//! wrote every other dirty level over its memory back first (a read-back,
//! `WritebackCause::Overlap`), then refilled the bound one from memory.
//! The guest never reads those bytes on its CPU in between.
//!
//! With the mirror ([`Device::set_alias_mirror`], on by default with lazy
//! render-to-texture on a backend that has one, `H2_D3D_ALIAS_MIRROR=0`
//! turns it off), the backend keeps a buffer standing for a range of guest
//! memory ([`Backend::mirror_reserve`]). The overlapping dirty level is
//! encoded into it as its guest texels ([`Backend::mirror_store`]: the
//! bytes a write-back would store, pitch padding as zeros) and becomes
//! clean; the levels over those bytes are refilled from it
//! ([`Backend::mirror_load`]: the fill a write-back then an upload would
//! make). Nothing waits.
//!
//! The device tracks, per byte of the range, whether the mirror holds it
//! newer than guest memory, and the guest bytes when it took them:
//!
//! * A guest store into such a byte (an exact [`Device::note_write`] range,
//!   or a byte that differs from what memory held when the mirror took it,
//!   as for lazy levels) wins: memory is newer there again.
//! * A draw into a level makes the level newer over its whole range (it was
//!   filled from the mirror first), so the mirror gives those bytes up.
//! * Where the guest may read the memory, exactly where dirty levels are
//!   written back lazily (a wait or busy check on an overlapping resource,
//!   `BlockOnTime`, push-buffer callbacks, a sample through guest memory,
//!   a destroy, `PersistDisplay`, teardown), the newer bytes are read back
//!   ([`Backend::mirror_read`]) and stored into guest memory, guest stores
//!   excepted.
//! * A level the mirror cannot fill on the GPU (a depth level) takes the
//!   bytes through guest memory: the overlap is flushed, then it is filled
//!   as before.
//!
//! Anything the mirror does not cover (formats other than the 32-bit word
//! formats and `D24S8`, unaligned levels, a level drawn into that overlaps
//! another one drawn into, guest stores waiting to be merged into the
//! level) goes through the write-back as before.

use super::*;
use crate::backend::GuestLayoutKind;

/// The largest range the mirror covers.
const MAX_MIRROR: u64 = 64 << 20;

/// What the backend's mirror holds, by guest byte.
pub(crate) struct AliasMirror {
    /// Guest virtual address of mirror byte 0.
    base: u32,
    /// The mirror holds this byte newer than guest memory.
    newer: Vec<bool>,
    /// Guest memory's byte when the mirror took it.
    synced: Vec<u8>,
    /// An exact guest store reached this byte since the mirror took it.
    stored: Vec<bool>,
    /// Write-tracker generation of the oldest take still holding newer
    /// bytes (pages written after it may hold guest stores).
    since: u64,
}

impl AliasMirror {
    fn end(&self) -> u64 {
        self.base as u64 + self.newer.len() as u64
    }

    /// `lo..hi` clamped to the mirror, as indices.
    fn span(&self, lo: u64, hi: u64) -> Option<(usize, usize)> {
        let (a, b) = (lo.max(self.base as u64), hi.min(self.end()));
        (a < b).then(|| {
            (
                (a - self.base as u64) as usize,
                (b - self.base as u64) as usize,
            )
        })
    }

    fn any_newer(&self, lo: u64, hi: u64) -> bool {
        self.span(lo, hi)
            .is_some_and(|(a, b)| self.newer[a..b].contains(&true))
    }
}

/// A level's guest bytes, `[lo, hi)`.
fn range<S>(t: &RttTarget<S>) -> (u64, u64) {
    (t.data as u64, t.data as u64 + t.len() as u64)
}

fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Runs of `true` in `mask` as `(start, end)`.
fn runs(mask: &[bool]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        if mask[i] {
            let s = i;
            while i < mask.len() && mask[i] {
                i += 1;
            }
            out.push((s, i));
        } else {
            i += 1;
        }
    }
    out
}

impl<B: Backend> Device<B> {
    /// Overlapping render-to-texture levels move through the backend's
    /// alias mirror instead of guest memory (on by default;
    /// `H2_D3D_ALIAS_MIRROR=0` turns it off; `docs/d3d.md`, "Overlapping
    /// levels on the GPU"). Needs lazy render-to-texture and a backend with
    /// a mirror ([`Backend::can_mirror`]). Turning it off flushes nothing:
    /// set it before the first draw.
    pub fn set_alias_mirror(&mut self, on: bool) {
        self.alias_mirror = on;
    }

    fn mirror_on(&self) -> bool {
        self.alias_mirror && self.rtt_lazy() && self.backend.can_mirror()
    }

    /// Word-aligned level in a format the mirror encodes.
    fn mirror_storable(&self, t: &RttTarget<B::Surface>) -> bool {
        let aligned = t.data.is_multiple_of(4)
            && match t.layout {
                GuestLayoutKind::Linear { pitch } => pitch.is_multiple_of(4),
                GuestLayoutKind::Swizzled => true,
            };
        let format = if t.depth {
            matches!(t.format, D3DFormat::D24S8 | D3DFormat::LinD24S8)
                && self.backend.can_sample_depth_surfaces()
        } else {
            gfx::guest_words::WordFormat::of(t.format).is_some()
        };
        aligned && format
    }

    /// Word-aligned colour level in a format the mirror decodes.
    fn mirror_loadable(&self, t: &RttTarget<B::Surface>) -> bool {
        !t.depth && self.mirror_storable(t)
    }

    /// Makes the mirror cover `lo..hi`: the first time, or after flushing
    /// what it holds when the range moves. `false`: too large.
    fn mirror_region(
        &mut self,
        mem: &mut dyn GuestMemory,
        lo: u64,
        hi: u64,
    ) -> Result<bool, D3dError> {
        if let Some(m) = &self.mirror
            && m.base as u64 <= lo
            && hi <= m.end()
        {
            return Ok(true);
        }
        let (mut a, mut b) = (lo & !0xFFF, hi.next_multiple_of(0x1000));
        if let Some(m) = &self.mirror {
            let (ua, ub) = (a.min(m.base as u64), b.max(m.end()));
            if ub - ua <= MAX_MIRROR {
                (a, b) = (ua, ub);
            }
        }
        if b - a > MAX_MIRROR || b > u32::MAX as u64 + 1 {
            return Ok(false);
        }
        self.mirror_flush_all(mem)?;
        self.backend
            .mirror_reserve((b - a) as u32)
            .map_err(D3dError::Backend)?;
        let n = (b - a) as usize;
        self.mirror = Some(AliasMirror {
            base: a as u32,
            newer: vec![false; n],
            synced: vec![0; n],
            stored: vec![false; n],
            since: self.tracker.generation(),
        });
        Ok(true)
    }

    /// Before `header` is filled from or matched against its memory, the
    /// dirty levels `others` over it go into the mirror instead of guest
    /// memory. `false`: they cannot (see the module docs); the caller
    /// writes them back.
    pub(super) fn park_overlapping(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
        others: &[u32],
    ) -> Result<bool, D3dError> {
        if !self.mirror_on() || others.is_empty() {
            return Ok(false);
        }
        let b = *self.rtt.get(&header).expect("cached");
        if b.dirty {
            return Ok(false);
        }
        let levels: Vec<(u32, RttTarget<B::Surface>)> =
            others.iter().map(|h| (*h, self.rtt[h])).collect();
        for (i, (h, t)) in levels.iter().enumerate() {
            if !self.mirror_storable(t) {
                return Ok(false);
            }
            // Order between two levels drawn into over the same bytes
            // would matter: not here.
            if levels[i + 1..]
                .iter()
                .any(|(_, o)| overlap(range(o), range(t)))
            {
                return Ok(false);
            }
            if self.rtt.iter().any(|(oh, o)| {
                o.dirty && oh != h && !others.contains(oh) && overlap(range(o), range(t))
            }) {
                return Ok(false);
            }
        }
        for (h, _) in &levels {
            if self.rtt_guest_stores(mem, *h)?.is_some() {
                return Ok(false);
            }
        }
        let lo = levels
            .iter()
            .map(|(_, t)| range(t).0)
            .chain([range(&b).0])
            .min()
            .expect("one");
        let hi = levels
            .iter()
            .map(|(_, t)| range(t).1)
            .chain([range(&b).1])
            .max()
            .expect("one");
        if !self.mirror_region(mem, lo, hi)? {
            return Ok(false);
        }
        for (h, t) in levels {
            let (a, z) = range(&t);
            let base = self.mirror.as_ref().expect("made").base;
            let off = t.data - base;
            let len = t.len() as usize;
            if let GuestLayoutKind::Linear { pitch } = t.layout
                && pitch > t.width * t.format.bytes_per_texel().unwrap_or(4)
            {
                // The write-back stores the pitch padding as zeros.
                self.backend
                    .mirror_write(off, &vec![0; len])
                    .map_err(D3dError::Backend)?;
            }
            self.backend
                .mirror_store(t.surface, t.format, t.width, t.height, t.layout, off)
                .map_err(D3dError::Backend)?;
            let bytes = mem.read_vec(t.data, len)?;
            let generation = self.tracker.generation();
            let m = self.mirror.as_mut().expect("made");
            let (i, j) = m.span(a, z).expect("covered");
            if !m.newer.contains(&true) {
                m.since = generation;
            }
            m.newer[i..j].fill(true);
            m.stored[i..j].fill(false);
            m.synced[i..j].copy_from_slice(&bytes);
            let hash = if self.cfg.upload == UploadPolicy::Verify {
                hash_bytes(&bytes)
            } else {
                0
            };
            let level = self.rtt.get_mut(&h).expect("cached");
            level.dirty = false;
            level.generation = generation;
            level.hash = hash;
            self.rtt_synced.insert(h, bytes);
            self.rtt_stores.remove(&h);
            self.depth_level_tex.remove(&h);
            let stale: Vec<u32> = self
                .rtt
                .iter()
                .filter(|(oh, o)| **oh != h && overlap(range(o), (a, z)))
                .map(|(oh, _)| *oh)
                .collect();
            self.mirror_stale.extend(stale);
            self.stats.mirror_stores += 1;
        }
        Ok(true)
    }

    /// The bytes of `lo..hi` the mirror holds newer that the guest stored
    /// into since (memory wins there), per byte of the mirror's span; `None`
    /// if nothing is newer there.
    fn mirror_guest_bytes(
        &self,
        mem: &dyn GuestMemory,
        lo: u64,
        hi: u64,
    ) -> Result<Option<(usize, Vec<bool>)>, D3dError> {
        let Some(m) = &self.mirror else {
            return Ok(None);
        };
        let Some((i, j)) = m.span(lo, hi) else {
            return Ok(None);
        };
        if !m.newer[i..j].contains(&true) {
            return Ok(None);
        }
        let mut cpu = vec![false; j - i];
        let addr = m.base + i as u32;
        // Only tracked uploads trust the tracker to see every store.
        let paged = self.cfg.upload != UploadPolicy::Tracked
            || self.tracker.dirty_since(addr, (j - i) as u32, m.since);
        let bytes = if paged {
            Some(mem.read_vec(addr, j - i)?)
        } else {
            None
        };
        for k in 0..j - i {
            let n = i + k;
            cpu[k] =
                m.newer[n] && (m.stored[n] || bytes.as_ref().is_some_and(|b| b[k] != m.synced[n]));
        }
        Ok(Some((i, cpu)))
    }

    /// Guest stores into bytes the mirror holds newer win: memory is newer
    /// there again. A word the guest stored only part of is flushed, so the
    /// mirror's newer bytes stay whole words.
    fn mirror_consolidate(
        &mut self,
        mem: &mut dyn GuestMemory,
        lo: u64,
        hi: u64,
    ) -> Result<(), D3dError> {
        let Some((i, cpu)) = self.mirror_guest_bytes(mem, lo, hi)? else {
            return Ok(());
        };
        if !cpu.contains(&true) {
            return Ok(());
        }
        let m = self.mirror.as_ref().expect("checked");
        let mixed = (0..cpu.len()).any(|k| {
            let n = i + k;
            let w = n & !3;
            cpu[k]
                && (w..w + 4).any(|x| {
                    x < m.newer.len() && m.newer[x] && !(x >= i && x - i < cpu.len() && cpu[x - i])
                })
        });
        if mixed {
            return self.mirror_flush_range(mem, lo, hi);
        }
        let m = self.mirror.as_mut().expect("checked");
        for (k, c) in cpu.iter().enumerate() {
            if *c {
                m.newer[i + k] = false;
                m.stored[i + k] = false;
            }
        }
        Ok(())
    }

    /// Stores the bytes of `lo..hi` the mirror holds newer into guest
    /// memory (one read-back), except where the guest stored since.
    pub(super) fn mirror_flush_range(
        &mut self,
        mem: &mut dyn GuestMemory,
        lo: u64,
        hi: u64,
    ) -> Result<(), D3dError> {
        let Some((i, cpu)) = self.mirror_guest_bytes(mem, lo, hi)? else {
            return Ok(());
        };
        let m = self.mirror.as_ref().expect("checked");
        let want: Vec<bool> = cpu
            .iter()
            .enumerate()
            .map(|(k, c)| m.newer[i + k] && !c)
            .collect();
        let base = m.base;
        let n = cpu.len();
        if let (Some(first), Some(last)) =
            (want.iter().position(|w| *w), want.iter().rposition(|w| *w))
        {
            let (a, z) = ((i + first) & !3, (i + last + 1).next_multiple_of(4));
            let bytes = self
                .backend
                .mirror_read(a as u32, (z - a) as u32)
                .map_err(D3dError::Backend)?;
            for (s, e) in runs(&want) {
                let (s, e) = (i + s, i + e);
                mem.write(base + s as u32, &bytes[s - a..e - a])?;
            }
            self.tracker.note_write(base + a as u32, (z - a) as u32);
            self.stats.mirror_flushes += 1;
        }
        let m = self.mirror.as_mut().expect("checked");
        m.newer[i..i + n].fill(false);
        m.stored[i..i + n].fill(false);
        Ok(())
    }

    /// [`mirror_flush_range`](Self::mirror_flush_range) of everything.
    pub(super) fn mirror_flush_all(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let Some(m) = &self.mirror else { return Ok(()) };
        let (lo, hi) = (m.base as u64, m.end());
        self.mirror_flush_range(mem, lo, hi)
    }

    /// A level is drawn into: it holds its whole range newer than the
    /// mirror (it was filled from it).
    pub(super) fn mirror_supersede(&mut self, header: u32) {
        let (Some(t), Some(m)) = (self.rtt.get(&header), self.mirror.as_mut()) else {
            return;
        };
        if let Some((i, j)) = m.span(range(t).0, range(t).1) {
            m.newer[i..j].fill(false);
            m.stored[i..j].fill(false);
        }
    }

    /// An exact guest store: bytes the mirror holds newer are the guest's.
    pub(super) fn note_mirror_store(&mut self, addr: u32, len: u32) {
        let Some(m) = self.mirror.as_mut() else {
            return;
        };
        if let Some((i, j)) = m.span(addr as u64, addr as u64 + len as u64) {
            for n in i..j {
                if m.newer[n] {
                    m.stored[n] = true;
                }
            }
        }
    }

    /// Fills level `header`'s host surface with its memory's current
    /// bytes, taking those the mirror holds newer from the mirror (on the
    /// backend when the level is a colour one it decodes, else through
    /// guest memory after a flush).
    pub(super) fn fill_level(
        &mut self,
        mem: &mut dyn GuestMemory,
        header: u32,
    ) -> Result<(), D3dError> {
        let t = *self.rtt.get(&header).expect("cached");
        let (lo, hi) = range(&t);
        self.mirror_stale.remove(&header);
        if self.mirror.as_ref().is_some_and(|m| m.any_newer(lo, hi)) {
            self.mirror_consolidate(mem, lo, hi)?;
        }
        if let Some(m) = self.mirror.as_ref().filter(|m| m.any_newer(lo, hi)) {
            let covered = m.base as u64 <= lo && hi <= m.end();
            if covered && self.mirror_loadable(&t) {
                let (i, j) = m.span(lo, hi).expect("covered");
                let gaps = runs(&m.newer[i..j].iter().map(|n| !n).collect::<Vec<_>>());
                let base = m.base;
                let aligned = gaps
                    .iter()
                    .all(|(s, e)| (i + s).is_multiple_of(4) && (i + e).is_multiple_of(4));
                if aligned {
                    let bytes = mem.read_vec(t.data, t.len() as usize)?;
                    for (s, e) in gaps {
                        self.backend
                            .mirror_write((i + s) as u32, &bytes[s..e])
                            .map_err(D3dError::Backend)?;
                    }
                    let off = t.data - base;
                    self.backend
                        .mirror_load(t.surface, t.format, t.width, t.height, t.layout, off)
                        .map_err(D3dError::Backend)?;
                    self.stats.mirror_loads += 1;
                    let hash = if self.cfg.upload == UploadPolicy::Verify {
                        hash_bytes(&bytes)
                    } else {
                        0
                    };
                    let generation = self.tracker.generation();
                    let level = self.rtt.get_mut(&header).expect("cached");
                    level.generation = generation;
                    level.hash = hash;
                    self.rtt_synced.insert(header, bytes);
                    self.rtt_stores.remove(&header);
                    self.depth_level_tex.remove(&header);
                    return Ok(());
                }
            }
            self.mirror_flush_range(mem, lo, hi)?;
        }
        let bytes = mem.read_vec(t.data, t.len() as usize)?;
        self.fill_rtt(header, bytes)
    }
}
