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
//! * A `D24S8` depth level is filled from the mirror on the GPU too
//!   ([`Device::set_alias_mirror_depth`], `H2_D3D_MIRROR_DEPTH=0` turns it
//!   off): the depth `z / 16777215` with the CPU decode's rounding, the
//!   stencil the word's low byte. Before, binding the shadow map after the
//!   colour levels over it were drawn into read the mirror back (the
//!   render thread waited), stored it into guest memory and uploaded it
//!   again, every time. A level the mirror cannot fill on the GPU (with
//!   that off) takes the bytes through guest memory: the overlap is
//!   flushed, then it is filled as before.
//!
//! The per-byte state is kept as bytes, so the scans on every binding
//! (anything newer over a level, where memory is newer) run eight bytes at
//! a time, and guest memory is compared with what the mirror took only on
//! pages the write tracker saw written since (tracked uploads; every page
//! otherwise), and byte by byte only where a page differs.
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
    /// 1: the mirror holds this byte newer than guest memory (bytes, so
    /// scans are `memchr` and eight at a time).
    newer: Vec<u8>,
    /// Guest memory's byte when the mirror took it.
    synced: Vec<u8>,
    /// 1: an exact guest store reached this byte since the mirror took it.
    stored: Vec<u8>,
    /// Some byte of `stored` may be 1 (else nothing need scan it).
    stored_any: bool,
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
            .is_some_and(|(a, b)| self.newer[a..b].contains(&1))
    }
}

/// A level's guest bytes, `[lo, hi)`.
fn range<S>(t: &RttTarget<S>) -> (u64, u64) {
    (t.data as u64, t.data as u64 + t.len() as u64)
}

fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The first index in `from..to` where `mask` (bytes 0 or 1) is `v`,
/// skipping eight bytes at a time.
fn find(mask: &[u8], from: usize, to: usize, v: u8) -> Option<usize> {
    let s = &mask[from..to];
    let other = if v == 1 {
        0
    } else {
        u64::from_ne_bytes([1; 8])
    };
    let (chunks, _) = s.as_chunks::<8>();
    let skip = chunks
        .iter()
        .position(|c| u64::from_ne_bytes(*c) != other)
        .unwrap_or(chunks.len())
        * 8;
    s[skip..]
        .iter()
        .position(|&b| b == v)
        .map(|k| from + skip + k)
}

/// Runs of `v` in `mask[from..to]` as `(start, end)` indices into `mask`.
fn runs(mask: &[u8], from: usize, to: usize, v: u8) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = from;
    while let Some(s) = find(mask, i, to, v) {
        let e = find(mask, s, to, v ^ 1).unwrap_or(to);
        out.push((s, e));
        i = e;
    }
    out
}

/// Whether index `x` is in one of the sorted, disjoint `runs`.
fn in_runs(runs: &[(usize, usize)], x: usize) -> bool {
    let k = runs.partition_point(|&(_, e)| e <= x);
    runs.get(k).is_some_and(|&(s, _)| s <= x)
}

/// Guest memory is compared with what the mirror took a page at a time.
const PAGE: usize = 0x1000;

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

    /// Depth levels over bytes the mirror holds are filled from it on the
    /// GPU (on by default; `H2_D3D_MIRROR_DEPTH=0` turns it off): binding
    /// the shadow map after the colour levels over it were drawn into no
    /// longer reads the mirror back and uploads it again.
    pub fn set_alias_mirror_depth(&mut self, on: bool) {
        self.mirror_depth = on;
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

    /// Word-aligned level in a format the mirror decodes: colour, or
    /// `D24S8` depth ([`Device::set_alias_mirror_depth`]).
    fn mirror_loadable(&self, t: &RttTarget<B::Surface>) -> bool {
        (!t.depth || self.mirror_depth) && self.mirror_storable(t)
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
            newer: vec![0; n],
            synced: vec![0; n],
            stored: vec![0; n],
            stored_any: false,
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
            if !m.newer.contains(&1) {
                m.since = generation;
            }
            m.newer[i..j].fill(1);
            m.stored[i..j].fill(0);
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
    /// into since (memory wins there), as sorted runs of mirror indices;
    /// `None` if nothing is newer there. Only pages the guest may have
    /// written (with tracked uploads, those the tracker saw written since
    /// the oldest take; else every page) are read and compared, and only
    /// where they differ from what the mirror took, byte by byte.
    fn mirror_guest_bytes(
        &self,
        mem: &dyn GuestMemory,
        lo: u64,
        hi: u64,
    ) -> Result<Option<Vec<(usize, usize)>>, D3dError> {
        let Some(m) = &self.mirror else {
            return Ok(None);
        };
        let Some((i, j)) = m.span(lo, hi) else {
            return Ok(None);
        };
        if !m.newer[i..j].contains(&1) {
            return Ok(None);
        }
        let mut owned: Vec<(usize, usize)> = Vec::new();
        let mut own = |n: usize| match owned.last_mut() {
            Some((_, e)) if *e == n => *e += 1,
            _ => owned.push((n, n + 1)),
        };
        // `base` is page-aligned: mirror pages are guest pages.
        let mut p = i;
        while p < j {
            let q = ((p / PAGE + 1) * PAGE).min(j);
            let (a, z) = (p, q);
            p = q;
            if !m.newer[a..z].contains(&1) {
                continue;
            }
            let addr = m.base + a as u32;
            // Only tracked uploads trust the tracker to see every store.
            let paged = self.cfg.upload != UploadPolicy::Tracked
                || self.tracker.dirty_since(addr, (z - a) as u32, m.since);
            let stored = m.stored_any && m.stored[a..z].contains(&1);
            let bytes = if paged {
                Some(mem.read_vec(addr, z - a)?)
            } else {
                None
            };
            let changed = bytes.as_ref().is_some_and(|b| b[..] != m.synced[a..z]);
            if !changed && !stored {
                continue;
            }
            for n in a..z {
                if m.newer[n] == 1
                    && (m.stored[n] == 1 || bytes.as_ref().is_some_and(|b| b[n - a] != m.synced[n]))
                {
                    own(n);
                }
            }
        }
        Ok(Some(owned))
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
        let Some(owned) = self.mirror_guest_bytes(mem, lo, hi)? else {
            return Ok(());
        };
        if owned.is_empty() {
            return Ok(());
        }
        let m = self.mirror.as_ref().expect("checked");
        // Within a run every byte is the guest's: only the words at its
        // ends can hold a newer byte the guest did not store.
        let mixed = owned.iter().any(|&(s, e)| {
            let (ws, we) = (s & !3, e.next_multiple_of(4).min(m.newer.len()));
            (ws..s)
                .chain(e..we)
                .any(|x| m.newer[x] == 1 && !in_runs(&owned, x))
        });
        if mixed {
            return self.mirror_flush_range(mem, lo, hi);
        }
        let m = self.mirror.as_mut().expect("checked");
        for (s, e) in owned {
            m.newer[s..e].fill(0);
            m.stored[s..e].fill(0);
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
        let Some(owned) = self.mirror_guest_bytes(mem, lo, hi)? else {
            return Ok(());
        };
        let m = self.mirror.as_mut().expect("checked");
        let (i, j) = m.span(lo, hi).expect("newer bytes there");
        // The guest's bytes stay; the rest of what is newer goes to memory.
        for &(s, e) in &owned {
            m.newer[s..e].fill(0);
        }
        let want = runs(&m.newer, i, j, 1);
        let base = m.base;
        if let (Some(&(first, _)), Some(&(_, last))) = (want.first(), want.last()) {
            let (a, z) = (first & !3, last.next_multiple_of(4));
            let bytes = self
                .backend
                .mirror_read(a as u32, (z - a) as u32)
                .map_err(D3dError::Backend)?;
            for (s, e) in want {
                mem.write(base + s as u32, &bytes[s - a..e - a])?;
            }
            self.tracker.note_write(base + a as u32, (z - a) as u32);
            self.stats.mirror_flushes += 1;
        }
        let m = self.mirror.as_mut().expect("checked");
        m.newer[i..j].fill(0);
        m.stored[i..j].fill(0);
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
            m.newer[i..j].fill(0);
            m.stored[i..j].fill(0);
        }
    }

    /// An exact guest store: bytes the mirror holds newer are the guest's.
    pub(super) fn note_mirror_store(&mut self, addr: u32, len: u32) {
        let Some(m) = self.mirror.as_mut() else {
            return;
        };
        if let Some((i, j)) = m.span(addr as u64, addr as u64 + len as u64) {
            for n in i..j {
                if m.newer[n] == 1 {
                    m.stored[n] = 1;
                    m.stored_any = true;
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
                // Where memory is newer, the mirror takes its bytes first.
                let gaps: Vec<(usize, usize)> = runs(&m.newer, i, j, 0)
                    .into_iter()
                    .map(|(s, e)| (s - i, e - i))
                    .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(mask: &[u8], from: usize, to: usize, v: u8) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (n, &b) in mask.iter().enumerate().take(to).skip(from) {
            if b == v {
                match out.last_mut() {
                    Some((_, e)) if *e == n => *e += 1,
                    _ => out.push((n, n + 1)),
                }
            }
        }
        out
    }

    #[test]
    fn runs_match_a_byte_by_byte_scan() {
        let mut s = 0x1234_5678u32;
        for len in [0usize, 1, 7, 8, 9, 63, 64, 65, 300, 4099] {
            for density in [0u32, 1, 8, 128, 250, 256] {
                let mask: Vec<u8> = (0..len)
                    .map(|_| {
                        s ^= s << 13;
                        s ^= s >> 17;
                        s ^= s << 5;
                        u8::from(s % 256 < density)
                    })
                    .collect();
                for (from, to) in [(0, len), (len / 3, len), (len / 5, len - len / 4)] {
                    for v in [0, 1] {
                        let want = naive(&mask, from, to, v);
                        assert_eq!(
                            runs(&mask, from, to, v),
                            want,
                            "{len} {density} {from}..{to} {v}"
                        );
                        for (x, &b) in mask.iter().enumerate().take(to).skip(from) {
                            assert_eq!(in_runs(&want, x), b == v);
                        }
                    }
                }
            }
        }
    }
}
