//! The layer draw's open shader (docs/graphics/retire-d3d-remainders.md,
//! "The open shader"): a layer draw whose items are all native but whose
//! shader-draw cache holds an open shader returns once its restore list
//! is written.
//!
//! The body's `0x00016b10` calls `0x00018900` when the cache's `+0x24`
//! (an implementation) is set: for each of the implementation's restore
//! entries (the `u16` at `+0x11c`: first `& 0x1ff`, count `>> 9`; five
//! bytes each in the cached descriptor's `+0x14` table, a state byte and
//! a value) it writes the state as `SetRenderState` does inline
//! ([`Write`]). Then the rest of the body's writes follow
//! (`frame_step::layer::writes`).
//!
//! - **The replay.** The host writes the list through the device as the
//!   guest's calls would ([`crate::native_layers::Host::restore`]).
//! - **The reference.** The layer draw calls the guest's `0x00018900`
//!   itself, then returns.
//! - **The gate.** Until it answers, every call takes the reference; then
//!   one call in [`SAMPLE`]. Each reference is compared with the
//!   prediction ([`predict`]): the shadow word of every listed state and
//!   D3D's dirty word, a mismatch counted by field. After [`CLEAN`] clean
//!   references in a row the gate answers and the other calls replay; at
//!   any difference it stops, until another clean run.
//!
//! Counted in `render.scene` as `open_shader_<outcome>`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::states::{DIRTY, HEADERS, MASKS, SHADOW};

use crate::frame_step::layer::{SETUP, SETUP_OPEN};

/// `0x00018900`: stdcall (context, restore range), `ret 8`.
pub const RESTORE: u32 = 0x0001_8900;
/// The cache's pass descriptor.
pub const SETUP_DESCRIPTOR: u32 = 0x20;
/// The implementation's restore range.
pub const IMPLEMENTATION_RESTORE: u32 = 0x11c;
/// The descriptor's render-state table.
pub const DESCRIPTOR_STATES: u32 = 0x14;
/// Bytes a table entry takes.
pub const ENTRY: u32 = 5;
/// The setters of states `0x88` and up, by state.
pub const SETTERS: u32 = 0x0040_38d8;
/// One call in this many is compared once the gate answers.
pub const SAMPLE: u64 = 64;
/// Clean references in a row before the gate answers.
pub const CLEAN: u32 = 16;

/// One restore entry as `0x00018900` applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Below `0x5c`: `SetRenderState_Simple` (`0x003f5cf0`, ECX the push
    /// header from `0x0043e6f0`, EDX the value), then the shadow.
    Simple { state: u8, header: u32, value: u32 },
    /// Below `0x88`: the state's mask (`0x0043e4d0`) into the dirty word,
    /// then the shadow.
    Deferred { state: u8, mask: u32, value: u32 },
    /// `0x88` and up: the state's stdcall setter (`0x004038d8`).
    Setter { state: u8, setter: u32, value: u32 },
}

impl Write {
    pub fn state(self) -> u8 {
        match self {
            Write::Simple { state, .. } | Write::Deferred { state, .. } | Write::Setter { state, .. } => state,
        }
    }

    pub fn value(self) -> u32 {
        match self {
            Write::Simple { value, .. } | Write::Deferred { value, .. } | Write::Setter { value, .. } => value,
        }
    }
}

/// The cache's open shader and its restore list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    /// The implementation (`[0x00485b48 + 0x24]`).
    pub implementation: u32,
    pub writes: Vec<Write>,
}

/// The open shader's restore list as `0x00018900` reads it; `None` when
/// the cache holds none.
pub fn read(mem: &dyn GuestMemory) -> Result<Option<Restore>, MemFault> {
    let implementation = mem.u32(SETUP + SETUP_OPEN)?;
    if implementation == 0 {
        return Ok(None);
    }
    let range = u32::from(mem.u16(implementation.wrapping_add(IMPLEMENTATION_RESTORE))?);
    let descriptor = mem.u32(SETUP + SETUP_DESCRIPTOR)?;
    let table = mem.u32(descriptor.wrapping_add(DESCRIPTOR_STATES))?;
    let mut at = table.wrapping_add(ENTRY * (range & 0x1ff));
    let mut writes = Vec::with_capacity((range >> 9) as usize);
    for _ in 0..range >> 9 {
        let state = mem.u8(at)?;
        let value = mem.u32(at.wrapping_add(1))?;
        let s = u32::from(state);
        writes.push(if s < 0x5c {
            Write::Simple { state, header: mem.u32(HEADERS + 4 * s)?, value }
        } else if s < 0x88 {
            Write::Deferred { state, mask: mem.u32(MASKS + 4 * s)?, value }
        } else {
            Write::Setter { state, setter: mem.u32(SETTERS + 4 * s)?, value }
        });
        at = at.wrapping_add(ENTRY);
    }
    Ok(Some(Restore { implementation, writes }))
}

/// One predicted word: its field name, address and value.
pub type Word = (String, u32, u32);

/// The words the list leaves: each listed state's shadow
/// (`0x00407498 + 4·state`, the last value written; a setter stores its
/// own state's) and the dirty word with every deferred state's mask.
pub fn predict(mem: &dyn GuestMemory, restore: &Restore) -> Result<Vec<Word>, MemFault> {
    let mut shadow = BTreeMap::new();
    let mut dirty = mem.u32(DIRTY)?;
    for w in &restore.writes {
        if let Write::Deferred { mask, .. } = *w {
            dirty |= mask;
        }
        shadow.insert(w.state(), w.value());
    }
    let mut out: Vec<Word> =
        shadow.into_iter().map(|(s, v)| (format!("state_{s:02x}"), SHADOW + 4 * u32::from(s), v)).collect();
    out.push(("dirty".into(), DIRTY, dirty));
    Ok(out)
}

/// The fields of `predicted` guest memory does not hold.
pub fn differing(mem: &dyn GuestMemory, predicted: &[Word]) -> Vec<String> {
    predicted
        .iter()
        .filter(|(_, at, v)| mem.u32(*at).ok() != Some(*v))
        .map(|(name, ..)| name.clone())
        .collect()
}

/// When the layer draw writes the list itself.
#[derive(Debug, Default)]
pub struct Gate {
    calls: u64,
    streak: u32,
    answering: bool,
}

impl Gate {
    /// Whether this call takes the reference: every call until the gate
    /// answers, then one in [`SAMPLE`].
    pub fn reference(&mut self) -> bool {
        self.calls += 1;
        !self.answering || self.calls % SAMPLE == 0
    }

    /// One reference compared: the fields that differed (none: it matched).
    pub fn compared(&mut self, differed: &[String]) {
        if differed.is_empty() {
            count("matched", 1);
            self.streak += 1;
            if self.streak >= CLEAN && !self.answering {
                self.answering = true;
                count("opened", 1);
            }
            return;
        }
        for field in differed {
            count(&format!("mismatched_{field}"), 1);
        }
        self.streak = 0;
        if self.answering {
            self.answering = false;
            count("fallback", 1);
        }
    }

    pub fn answering(&self) -> bool {
        self.answering
    }
}

static GATE: Mutex<Gate> = Mutex::new(Gate { calls: 0, streak: 0, answering: false });
static COUNTS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// `open_shader_<outcome>`.
pub fn count(outcome: &str, n: u64) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(format!("open_shader_{outcome}")).or_default() += n;
}

pub fn counters() -> Vec<(String, u64)> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (k.clone(), v)).collect()
}

/// How a call restores the list.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// The host wrote it.
    Replayed,
    /// Call the guest's [`RESTORE`], then compare with these words.
    Reference(Vec<Word>),
}

/// This call's restore of `restore`: the replay through `replay` (the
/// host's, [`crate::native_layers::Host::restore`]) when the gate
/// answers, else the reference with its prediction. `Err` when the
/// prediction does not read (the guest body runs). A declined replay
/// takes the reference, counted `declined_<why>`.
pub fn plan(
    mem: &mut dyn GuestMemory,
    restore: &Restore,
    replay: impl FnOnce(&mut dyn GuestMemory, &[Write]) -> Result<(), &'static str>,
) -> Result<Plan, MemFault> {
    let reference = GATE.lock().unwrap_or_else(|e| e.into_inner()).reference();
    if !reference {
        match replay(mem, &restore.writes) {
            Ok(()) => {
                count("replayed", 1);
                return Ok(Plan::Replayed);
            }
            Err(why) => count(&format!("declined_{why}"), 1),
        }
    }
    let predicted = predict(&*mem, restore)?;
    count("referenced", 1);
    Ok(Plan::Reference(predicted))
}

/// After the guest's [`RESTORE`]: compares its words with `predicted`.
pub fn compare(mem: &dyn GuestMemory, predicted: &[Word]) {
    let differed = differing(mem, predicted);
    GATE.lock().unwrap_or_else(|e| e.into_inner()).compared(&differed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    const DESCRIPTOR: u32 = 0x0045_1000;
    const TABLE: u32 = 0x0045_2000;
    const IMPLEMENTATION: u32 = 0x0045_3000;

    /// An open shader whose restore list is entries 2..5 of the table: a
    /// simple state, a deferred one and a setter's, the simple one twice.
    fn open() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x10_0000);
        m.set_u32(SETUP + SETUP_OPEN, IMPLEMENTATION).unwrap();
        m.set_u32(SETUP + SETUP_DESCRIPTOR, DESCRIPTOR).unwrap();
        m.set_u32(DESCRIPTOR + DESCRIPTOR_STATES, TABLE).unwrap();
        m.set_u16(IMPLEMENTATION + IMPLEMENTATION_RESTORE, 4 << 9 | 2).unwrap();
        for (n, (state, value)) in [(0u8, 9), (0, 9), (0x10, 1), (0x60, 7), (0x8f, 3), (0x10, 5)].into_iter().enumerate() {
            m.set_u8(TABLE + ENTRY * n as u32, state).unwrap();
            m.set_u32(TABLE + ENTRY * n as u32 + 1, value).unwrap();
        }
        m.set_u32(HEADERS + 4 * 0x10, 0x0004_0300).unwrap();
        m.set_u32(MASKS + 4 * 0x60, 0x20).unwrap();
        m.set_u32(SETTERS + 4 * 0x8f, 0x003f_54f0).unwrap();
        m.set_u32(DIRTY, 0x101).unwrap();
        m
    }

    #[test]
    fn the_restore_list_is_read_and_classed_as_the_original_applies_it() {
        let m = open();
        let r = read(&m).unwrap().unwrap();
        assert_eq!(r.implementation, IMPLEMENTATION);
        assert_eq!(r.writes, [
            Write::Simple { state: 0x10, header: 0x0004_0300, value: 1 },
            Write::Deferred { state: 0x60, mask: 0x20, value: 7 },
            Write::Setter { state: 0x8f, setter: 0x003f_54f0, value: 3 },
            Write::Simple { state: 0x10, header: 0x0004_0300, value: 5 },
        ]);
        let mut closed = open();
        closed.set_u32(SETUP + SETUP_OPEN, 0).unwrap();
        assert_eq!(read(&closed).unwrap(), None, "no open shader");
    }

    #[test]
    fn the_prediction_is_each_states_last_value_and_the_dirty_masks() {
        let mut m = open();
        let r = read(&m).unwrap().unwrap();
        let p = predict(&m, &r).unwrap();
        assert_eq!(p, [
            ("state_10".to_owned(), SHADOW + 0x40, 5),
            ("state_60".to_owned(), SHADOW + 0x180, 7),
            ("state_8f".to_owned(), SHADOW + 0x23c, 3),
            ("dirty".to_owned(), DIRTY, 0x121),
        ]);
        // The guest's restore, as the original writes memory.
        for (at, v) in [(SHADOW + 0x40, 5), (SHADOW + 0x180, 7), (SHADOW + 0x23c, 3), (DIRTY, 0x121)] {
            m.set_u32(at, v).unwrap();
        }
        assert!(differing(&m, &p).is_empty());
        m.set_u32(SHADOW + 0x23c, 4).unwrap();
        assert_eq!(differing(&m, &p), ["state_8f"], "a setter that stored another slot");
    }

    #[test]
    fn the_gate_answers_after_clean_references_and_stops_at_a_difference() {
        let mut g = Gate::default();
        for _ in 0..CLEAN {
            assert!(g.reference(), "every call before it answers");
            g.compared(&[]);
        }
        assert!(g.answering());
        let references = (0..SAMPLE).filter(|_| g.reference()).count();
        assert_eq!(references, 1, "then one call in {SAMPLE}");
        g.compared(&["dirty".into()]);
        assert!(!g.answering(), "a difference stops it");
        assert!(g.reference());
    }

    #[test]
    fn a_declined_replay_takes_the_reference() {
        let mut m = open();
        let r = read(&m).unwrap().unwrap();
        // Open the shared gate.
        for _ in 0..CLEAN {
            GATE.lock().unwrap().compared(&[]);
        }
        let mut replays = 0;
        let mut declined = 0;
        for _ in 0..SAMPLE {
            match plan(&mut m, &r, |_, writes| {
                assert_eq!(writes.len(), 4);
                Err("setter")
            })
            .unwrap()
            {
                Plan::Replayed => replays += 1,
                Plan::Reference(p) => {
                    declined += 1;
                    assert_eq!(p.last().unwrap().0, "dirty");
                }
            }
        }
        assert_eq!((replays, declined), (0, SAMPLE as usize));
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["open_shader_declined_setter"] >= SAMPLE - 1);
    }
}
