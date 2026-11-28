//! The particle view stamps' home outside the arena
//! (docs/engine/sim-render-split.md, "6. Render scratch and stamps"):
//! what `0x00174420` writes to mark what a view saw, kept in host memory
//! keyed by datum, so the frame step leaves the particle arrays as the
//! simulation left them.
//!
//! Two stamps, as the model makes them ([`super::view_update`]):
//!
//! - system `+0xd` bit 2 (`+0xc` bit `0x400`): the system was forced
//!   visible. The update reads it back the next frame and children take
//!   their parent's.
//! - particle `+0x30`: the render frame it was last kept in.
//!
//! An entry holds the datum's salt (`+0`) and the arena's value when the
//! entry was made. A reused datum (another salt) or a tick write to the
//! field (another arena value) ends it, and the arena's value counts
//! again. Only values that differ from the arena's are held.
//!
//! Where the guest body still runs (a sampled frame, a closed gate, a
//! refusal), [`Stamps::lend`] puts the held values in the arena before it
//! and [`Stamps::take`] takes what it made and puts the arena's back after
//! it. Where the answer runs, the model reads through [`View`] and
//! [`Stamps::keep`] holds its stamps; nothing reaches the arena.

use std::collections::{BTreeMap, HashMap};

use guest_layout::memory::{GuestMemory, MemFault};

use super::view_update::{self, PARTICLE, PARTICLES, SYSTEM, SYSTEMS};

/// System byte that holds the forced bit, and the bit.
pub const FORCED_AT: u32 = 0xd;
pub const FORCED: u8 = 4;
/// The system's flags word, whose high byte is [`FORCED_AT`].
const FLAGS_AT: u32 = 0xc;
/// The particle's frame stamp.
pub const STAMP_AT: u32 = 0x30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry<T> {
    salt: u16,
    /// The arena's value when the entry was made.
    base: T,
    value: T,
}

/// The two arrays as they are now: data and live elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrays {
    systems: (u32, u32),
    particles: (u32, u32),
}

impl Arrays {
    pub fn read(m: &dyn GuestMemory) -> view_update::Result<Self> {
        let (s, sl) = view_update::array_span(m, SYSTEMS, SYSTEM)?;
        let (p, pl) = view_update::array_span(m, PARTICLES, PARTICLE)?;
        Ok(Self {
            systems: (s, sl / SYSTEM),
            particles: (p, pl / PARTICLE),
        })
    }

    fn element(span: (u32, u32), stride: u32, addr: u32) -> Option<(u32, u32)> {
        let off = addr.wrapping_sub(span.0);
        (off < span.1.wrapping_mul(stride)).then_some((off / stride, off % stride))
    }

    fn system(&self, addr: u32) -> Option<(u32, u32)> {
        Self::element(self.systems, SYSTEM, addr)
    }

    fn particle(&self, addr: u32) -> Option<(u32, u32)> {
        Self::element(self.particles, PARTICLE, addr)
    }

    fn system_at(&self, i: u32) -> u32 {
        self.systems.0.wrapping_add(i * SYSTEM)
    }

    fn particle_at(&self, i: u32) -> u32 {
        self.particles.0.wrapping_add(i * PARTICLE)
    }
}

/// The render's stamps, by element index.
#[derive(Debug, Default)]
pub struct Stamps {
    forced: HashMap<u32, Entry<bool>>,
    frames: HashMap<u32, Entry<u32>>,
}

/// The arena's stamps before a guest body ran, for [`Stamps::take`].
#[derive(Debug)]
pub struct Lent {
    arrays: Arrays,
    forced: Vec<bool>,
    frames: Vec<u32>,
}

impl Stamps {
    /// Drop every stamp (a snapshot load: the next frame makes them again).
    pub fn clear(&mut self) {
        self.forced.clear();
        self.frames.clear();
    }

    /// How many stamps differ from the arena's.
    #[cfg(test)]
    pub fn held(&self) -> usize {
        self.forced.len() + self.frames.len()
    }

    /// System `i`'s forced bit, if held and still current.
    fn forced(&self, m: &dyn GuestMemory, a: &Arrays, i: u32) -> Option<bool> {
        let e = self.forced.get(&i)?;
        let s = a.system_at(i);
        let arena = m.u8(s.wrapping_add(FORCED_AT)).ok()? & FORCED != 0;
        (m.u16(s).ok()? == e.salt && arena == e.base).then_some(e.value)
    }

    /// Particle `i`'s frame stamp, if held and still current.
    fn frame(&self, m: &dyn GuestMemory, a: &Arrays, i: u32) -> Option<u32> {
        let e = self.frames.get(&i)?;
        let p = a.particle_at(i);
        let arena = m.u32(p.wrapping_add(STAMP_AT)).ok()?;
        (m.u16(p).ok()? == e.salt && arena == e.base).then_some(e.value)
    }

    fn hold_forced(&mut self, m: &dyn GuestMemory, a: &Arrays, i: u32, value: bool) -> Result<(), MemFault> {
        let s = a.system_at(i);
        let base = m.u8(s.wrapping_add(FORCED_AT))? & FORCED != 0;
        if value == base {
            self.forced.remove(&i);
        } else {
            self.forced.insert(i, Entry { salt: m.u16(s)?, base, value });
        }
        Ok(())
    }

    fn hold_frame(&mut self, m: &dyn GuestMemory, a: &Arrays, i: u32, value: u32) -> Result<(), MemFault> {
        let p = a.particle_at(i);
        let base = m.u32(p.wrapping_add(STAMP_AT))?;
        if value == base {
            self.frames.remove(&i);
        } else {
            self.frames.insert(i, Entry { salt: m.u16(p)?, base, value });
        }
        Ok(())
    }

    /// The arena `m` with the held stamps over it: what the guest body
    /// would read if the stamps were still there.
    pub fn view<'a>(&'a self, m: &'a dyn GuestMemory) -> view_update::Result<View<'a>> {
        Ok(View {
            base: m,
            stamps: self,
            arrays: Arrays::read(m)?,
        })
    }

    /// Holds the stamps among `writes` (a model's, made through
    /// [`Stamps::view`] of the arena `m`) and returns the rest, for the
    /// arena.
    pub fn keep(&mut self, m: &dyn GuestMemory, writes: &BTreeMap<u32, u8>) -> view_update::Result<BTreeMap<u32, u8>> {
        let a = Arrays::read(m)?;
        let mut rest = BTreeMap::new();
        let mut frames: BTreeMap<u32, [Option<u8>; 4]> = BTreeMap::new();
        for (&addr, &b) in writes {
            if let Some((i, off)) = a.system(addr) {
                let arena = m.u8(addr)?;
                match off {
                    // `set_u16` of the flags rewrites their low byte as it was.
                    FLAGS_AT if b == arena => continue,
                    FORCED_AT => {
                        self.hold_forced(m, &a, i, b & FORCED != 0)?;
                        if b & !FORCED != arena & !FORCED {
                            rest.insert(addr, (arena & FORCED) | (b & !FORCED));
                        }
                        continue;
                    }
                    _ => {}
                }
            } else if let Some((i, off)) = a.particle(addr)
                && (STAMP_AT..STAMP_AT + 4).contains(&off)
            {
                frames.entry(i).or_default()[(off - STAMP_AT) as usize] = Some(b);
                continue;
            }
            rest.insert(addr, b);
        }
        for (i, bytes) in frames {
            let at = a.particle_at(i).wrapping_add(STAMP_AT);
            let mut now = self.view(m)?.u32(at)?.to_le_bytes();
            for (k, b) in bytes.iter().enumerate() {
                if let Some(b) = b {
                    now[k] = *b;
                }
            }
            self.hold_frame(m, &a, i, u32::from_le_bytes(now))?;
        }
        Ok(rest)
    }

    /// Puts the held stamps in the arena for a guest body, keeping what
    /// was there. `None` when the arrays cannot be read: the body then
    /// runs on the arena as it is.
    pub fn lend(&self, m: &mut dyn GuestMemory) -> Option<Lent> {
        let arrays = Arrays::read(&*m).ok()?;
        let mut lent = Lent {
            arrays,
            forced: Vec::with_capacity(arrays.systems.1 as usize),
            frames: Vec::with_capacity(arrays.particles.1 as usize),
        };
        for i in 0..arrays.systems.1 {
            let at = arrays.system_at(i).wrapping_add(FORCED_AT);
            let b = m.u8(at).ok()?;
            lent.forced.push(b & FORCED != 0);
        }
        for i in 0..arrays.particles.1 {
            lent.frames.push(m.u32(arrays.particle_at(i).wrapping_add(STAMP_AT)).ok()?);
        }
        for i in 0..arrays.systems.1 {
            if let Some(v) = self.forced(&*m, &arrays, i) {
                let at = arrays.system_at(i).wrapping_add(FORCED_AT);
                let b = m.u8(at).ok()?;
                m.set_u8(at, if v { b | FORCED } else { b & !FORCED }).ok()?;
            }
        }
        for i in 0..arrays.particles.1 {
            if let Some(v) = self.frame(&*m, &arrays, i) {
                m.set_u32(arrays.particle_at(i).wrapping_add(STAMP_AT), v).ok()?;
            }
        }
        Some(lent)
    }

    /// After the guest body: holds the stamps it left and puts the
    /// arena's from before [`Stamps::lend`] back.
    pub fn take(&mut self, m: &mut dyn GuestMemory, lent: Lent) {
        let a = lent.arrays;
        for (i, &was) in (0..).zip(&lent.forced) {
            let at = a.system_at(i).wrapping_add(FORCED_AT);
            let Ok(b) = m.u8(at) else { continue };
            let back = if was { b | FORCED } else { b & !FORCED };
            if m.set_u8(at, back).is_ok() {
                let _ = self.hold_forced(&*m, &a, i, b & FORCED != 0);
            }
        }
        for (i, &was) in (0..).zip(&lent.frames) {
            let at = a.particle_at(i).wrapping_add(STAMP_AT);
            let Ok(now) = m.u32(at) else { continue };
            if m.set_u32(at, was).is_ok() {
                let _ = self.hold_frame(&*m, &a, i, now);
            }
        }
    }
}

/// The arena with the held stamps laid over it. Read only: a model wraps
/// it in its own overlay.
pub struct View<'a> {
    base: &'a dyn GuestMemory,
    stamps: &'a Stamps,
    arrays: Arrays,
}

impl GuestMemory for View<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.base.read(addr, buf)?;
        for (k, b) in buf.iter_mut().enumerate() {
            let at = addr.wrapping_add(k as u32);
            if let Some((i, FORCED_AT)) = self.arrays.system(at) {
                if let Some(v) = self.stamps.forced(self.base, &self.arrays, i) {
                    *b = if v { *b | FORCED } else { *b & !FORCED };
                }
            } else if let Some((i, off)) = self.arrays.particle(at)
                && (STAMP_AT..STAMP_AT + 4).contains(&off)
                && let Some(v) = self.stamps.frame(self.base, &self.arrays, i)
            {
                *b = v.to_le_bytes()[(off - STAMP_AT) as usize];
            }
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

#[cfg(not(test))]
static STAMPS: std::sync::Mutex<Option<Stamps>> = std::sync::Mutex::new(None);

/// The process's stamps.
#[cfg(not(test))]
pub fn with<R>(f: impl FnOnce(&mut Stamps) -> R) -> R {
    f(STAMPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Stamps::default))
}

// Tests run in parallel: each keeps its own.
#[cfg(test)]
thread_local! {
    static STAMPS: std::cell::RefCell<Stamps> = std::cell::RefCell::new(Stamps::default());
}

#[cfg(test)]
pub fn with<R>(f: impl FnOnce(&mut Stamps) -> R) -> R {
    STAMPS.with(|s| f(&mut s.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::super::view_update::tests::{PART_DATA, SYS_DATA, particle, system, world};
    use super::super::view_update::{FRAME, predict};
    use super::*;
    use guest_layout::memory::FlatMemory;

    const CW: u16 = 0x027f;

    /// A forced system with one particle, at frame `frame`.
    fn forced_world(frame: u32) -> (FlatMemory, u32, u32) {
        let mut m = world();
        m.set_u32(FRAME, frame).unwrap();
        let s = system(&mut m, 0, 0);
        let p = particle(&mut m, 0, 4.0, u32::MAX);
        m.set_u16(s, 0x1234).unwrap();
        m.set_u16(p, 0x5678).unwrap();
        m.set_u16(s + FLAGS_AT, 0x0400).unwrap();
        (m, s, p)
    }

    #[test]
    fn an_answered_update_holds_its_stamps_and_leaves_the_arrays_alone() {
        let (mut m, s, p) = forced_world(77);
        // The tick cleared the forced bit; the render's view still has it.
        m.set_u8(s + FORCED_AT, 0).unwrap();
        let mut st = Stamps::default();
        st.hold_forced(&m, &Arrays::read(&m).unwrap(), 0, true).unwrap();
        let systems = m.bytes(SYS_DATA, 0x54 * 8).unwrap();
        let particles = m.bytes(PART_DATA, 0x34 * 8).unwrap();

        let prediction = predict(&st.view(&m).unwrap(), [0, 0, 0, 0], CW).unwrap();
        let rest = st.keep(&m, &prediction.writes).unwrap();
        for (&a, &b) in &rest {
            m.set_u8(a, b).unwrap();
        }
        assert_eq!(m.bytes(SYS_DATA, 0x54 * 8).unwrap(), systems, "no system byte reaches the arena");
        assert_eq!(m.bytes(PART_DATA, 0x34 * 8).unwrap(), particles, "no particle byte reaches the arena");
        let view = st.view(&m).unwrap();
        assert_eq!(view.u32(p + STAMP_AT).unwrap(), 77, "the stamp is the render's");
        assert_eq!(view.u8(s + FORCED_AT).unwrap() & FORCED, FORCED);
        assert_eq!(prediction.entries.len(), 1, "the forced bit from the table drew the particle");
        assert!(rest.keys().all(|a| a.wrapping_sub(SYS_DATA) >= 0x54 * 8));
    }

    #[test]
    fn a_reused_datum_or_a_tick_write_ends_the_stamp() {
        let (mut m, _, p) = forced_world(77);
        let mut st = Stamps::default();
        let a = Arrays::read(&m).unwrap();
        st.hold_frame(&m, &a, 0, 77).unwrap();
        assert_eq!(st.view(&m).unwrap().u32(p + STAMP_AT).unwrap(), 77);
        // The tick writes the stamp (a new particle's birth frame).
        m.set_u32(p + STAMP_AT, 80).unwrap();
        assert_eq!(st.view(&m).unwrap().u32(p + STAMP_AT).unwrap(), 80);
        // A held stamp whose datum is reused.
        m.set_u32(p + STAMP_AT, 0).unwrap();
        st.hold_frame(&m, &a, 0, 77).unwrap();
        m.set_u16(p, 0x5679).unwrap();
        assert_eq!(st.view(&m).unwrap().u32(p + STAMP_AT).unwrap(), 0);
        // A value equal to the arena's is not held.
        st.hold_frame(&m, &a, 0, 0).unwrap();
        assert_eq!(st.held(), 0);
    }

    #[test]
    fn a_guest_body_runs_on_the_lent_stamps_and_gives_them_back() {
        let (mut m, s, p) = forced_world(90);
        m.set_u8(s + FORCED_AT, 0).unwrap();
        let mut st = Stamps::default();
        st.hold_forced(&m, &Arrays::read(&m).unwrap(), 0, true).unwrap();
        let arena = m.bytes.clone();

        let lent = st.lend(&mut m).unwrap();
        assert_eq!(m.u8(s + FORCED_AT).unwrap() & FORCED, FORCED, "the body reads the render's bit");
        // The guest body, as the model makes it.
        let prediction = predict(&m, [0, 0, 0, 0], CW).unwrap();
        for (&a, &b) in &prediction.writes {
            m.set_u8(a, b).unwrap();
        }
        st.take(&mut m, lent);

        let at = (SYS_DATA - 0x0040_0000) as usize;
        assert_eq!(m.bytes(SYS_DATA, 0x54 * 8).unwrap(), &arena[at..at + 0x54 * 8]);
        assert_eq!(m.u32(p + STAMP_AT).unwrap(), 0, "the arena keeps the tick's stamp");
        let view = st.view(&m).unwrap();
        assert_eq!(view.u32(p + STAMP_AT).unwrap(), 90);
        assert_eq!(view.u8(s + FORCED_AT).unwrap() & FORCED, FORCED);
    }

    #[test]
    fn other_flag_bits_and_other_fields_still_reach_the_arena() {
        let (m, s, _) = forced_world(5);
        let mut st = Stamps::default();
        let writes = BTreeMap::from([
            (s + FLAGS_AT, 0x00),
            (s + FORCED_AT, FORCED | 0x01),
            (s + 0x10, 0xaa),
            (0x0064_0000, 0xbb),
        ]);
        let rest = st.keep(&m, &writes).unwrap();
        assert_eq!(
            rest,
            BTreeMap::from([(s + FORCED_AT, 0x05 & !FORCED | (m.u8(s + FORCED_AT).unwrap() & FORCED)), (s + 0x10, 0xaa), (0x0064_0000, 0xbb)])
        );
    }
}
