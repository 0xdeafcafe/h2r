//! The scene's dynamic light from the light frame body
//! (docs/graphics/native-light-gather.md, slice L5).
//!
//! The body hands each drawn light's passes call over just before it
//! makes it ([`handed`]). The passes' entry pairs the hand-off with its
//! registers and the light's serial ([`entered`]). The passes' first
//! call is the item lists `0x00040e90` (`0x00031192`, returning to
//! [`ITEMS_RETURN`]); when the item body makes them, the scene's light,
//! its flags and its steps are made from the hand-off and the layers of
//! the body's `LightItems` ([`Source::handed`], [`made`]), not read back
//! from the lists. When the guest makes the lists, the light is read at
//! the following constants call ([`Source::observed`]).
//!
//! Each light the scene takes from a hand-off is checked against guest
//! memory after its lists are made: `light_handoff_compared` per check,
//! `light_handoff_mismatch` per difference.

use std::cell::{Cell, RefCell};

use engine::light_constants as lc;
use engine::light_passes as lp;
use guest_layout::memory::GuestMemory;
use engine::render::light_gather::Refusal;
use engine::render::light_gather::frame::{Gathered, Passes};
use engine::render::light_gather::items::LightItems;
use native_render::light_passes::{self, CASTERS, LIGHT_LIST};
use timeline::native::{SceneList, SceneListResult};

use super::{Entry, Ro};

/// The return address of the passes' call of the item lists
/// (`0x00031192`, **E**).
pub const ITEMS_RETURN: u32 = 0x0003_1197;

/// A list's layer mask (`+8`).
const MASK: u32 = 8;

/// What the scene's light is made from: the `ligh` datum, the call the
/// passes make to `0x0004a1e0` (`0x000311c3`, its shadow word after step
/// 2 of "One light's passes"), and the layers of the light's two lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    /// EAX at the passes' entry.
    pub definition: u32,
    pub call: lc::Call,
    /// List 1's layers (`0x004c1a68`).
    pub receivers: u32,
    /// List 2's layers (`0x004c1a80`).
    pub casters: u32,
}

impl Source {
    /// From the body's hand-off and the layers of the lists the item body
    /// made. A set shadow byte becomes the casters' bit 10 (`0x0003119b`).
    /// `None` when the tag table does not read.
    pub fn handed(
        mem: &dyn GuestMemory,
        passes: &Passes,
        receivers: u32,
        casters: u32,
    ) -> Option<Self> {
        Some(Self {
            definition: passes.definition,
            call: lc::Call {
                frame: passes.position,
                context: passes.context,
                light: passes.light,
                definition: definition_data(mem, passes.definition)?,
                shadow: lp::shadow_argument(passes.shadow, casters),
                weight: passes.weight,
            },
            receivers,
            casters,
        })
    }

    /// Read from guest memory with the passes' registers `e`: their
    /// stack words, the scene flags and the two lists' masks. At the
    /// passes' entry the lists are the last light's; after the item lists'
    /// call they are this light's. `None` when a word does not read.
    pub fn observed(mem: &dyn GuestMemory, e: &Entry) -> Option<Self> {
        let arg = |offset: u32| mem.u32(e.esp.wrapping_add(offset)).ok();
        let scene = mem.u32(lp::global::SCENE_FLAGS).ok()?;
        Some(Self {
            definition: e.eax,
            call: lc::Call {
                frame: arg(8)?,
                context: e.ecx,
                light: arg(4)?,
                definition: definition_data(mem, e.eax)?,
                shadow: lp::shadow_argument(arg(0xc)?, scene),
                weight: arg(0x10)?,
            },
            receivers: mem.u32(LIGHT_LIST + MASK).ok()?,
            casters: mem.u32(CASTERS + MASK).ok()?,
        })
    }

    /// The steps' inputs with the passes' `flags` and the definition's:
    /// the lists' layers are this source's; the additional casters'
    /// condition is read (`light_passes::read`).
    pub fn inputs(
        &self,
        mem: &dyn GuestMemory,
        flags: u32,
        definition_flags: u32,
    ) -> Option<light_passes::Inputs> {
        let read = light_passes::read(&Ro(mem), flags, definition_flags)?;
        Some(light_passes::Inputs {
            receivers: self.receivers,
            casters: self.casters,
            ..read
        })
    }
}

/// The `ligh` definition's address for the datum `definition`, from the
/// tag table (`0x00031183`..`0x0003118c`).
fn definition_data(mem: &dyn GuestMemory, definition: u32) -> Option<u32> {
    let table = mem.u32(guest_layout::tags::TAG_ENTRIES_GLOBAL).ok()?;
    mem.u32(
        table
            .wrapping_add((definition & 0xffff) << 4)
            .wrapping_add(8),
    )
    .ok()
}

/// A hand-off: the passes' call, and once their entry is seen, its
/// registers and the light's serial.
#[derive(Clone, Copy, Debug)]
struct Handoff {
    passes: Passes,
    entered: Option<(Entry, u64)>,
}

/// A hand-off whose item lists are being made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Taken {
    pub passes: Passes,
    pub entry: Entry,
    pub serial: u64,
}

thread_local! {
    // The entry supplies the registers, but only the completed item lists
    // supply this light's masks. A successful Rust hand-off needs no reread.
    static OBSERVE: Cell<Option<(Entry, u64)>> = const { Cell::new(None) };
    /// The frame body's hand-off to its passes and item lists.
    static HANDOFF: RefCell<Option<Handoff>> = const { RefCell::new(None) };
}

/// The frame body hands `gathered` over, before its passes' call.
pub(super) fn handed(gathered: &Gathered) {
    hand(gathered.passes);
}

fn hand(passes: Passes) {
    let last = HANDOFF.replace(Some(Handoff {
        passes,
        entered: None,
    }));
    if last.is_some() {
        not_taken();
    }
}

/// At the passes' entry: `true` when the pending hand-off is this call's
/// (its registers and stack words), which then waits for the light's
/// item lists with `entry` and `serial`. Without a matching hand-off the
/// caller defers the scene read until the constants call.
pub(super) fn entered(mem: &dyn GuestMemory, entry: Entry, serial: u64) -> bool {
    OBSERVE.set(None);
    let Some(h) = HANDOFF.take() else {
        return false;
    };
    if h.entered.is_some() || !is_call(mem, &h.passes, &entry) {
        not_taken();
        return false;
    }
    HANDOFF.set(Some(Handoff {
        entered: Some((entry, serial)),
        ..h
    }));
    true
}

/// Whether `e` is the call `p` describes.
fn is_call(mem: &dyn GuestMemory, p: &Passes, e: &Entry) -> bool {
    e.eax == p.definition
        && e.ecx == p.context
        && p.args()
            .iter()
            .zip((4u32..).step_by(4))
            .all(|(&word, at)| mem.u32(e.esp.wrapping_add(at)).ok() == Some(word))
}

/// At `SCENE_ITEMS` returning to `ret`: the entered hand-off, when this is
/// its passes' call of the item lists.
pub(super) fn take(ret: u32) -> Option<Taken> {
    if ret != ITEMS_RETURN {
        return None;
    }
    let h = HANDOFF.take()?;
    match h.entered {
        Some((entry, serial)) => Some(Taken {
            passes: h.passes,
            entry,
            serial,
        }),
        None => {
            HANDOFF.set(Some(h));
            None
        }
    }
}

/// The item body made the light's lists: the scene's light from the
/// hand-off and `items`' layers, then the check against memory.
pub(super) fn made(mem: &dyn GuestMemory, taken: &Taken, items: &LightItems) {
    let Some(source) = Source::handed(
        mem,
        &taken.passes,
        items.receivers.layers,
        items.casters.layers,
    ) else {
        guest_made(taken, Refusal::Memory);
        return;
    };
    super::open_light(mem, &source, taken.entry.fpu(), taken.serial);
    SceneList::LightHandoff.count(SceneListResult::Native);
    SceneList::LightHandoff.count(SceneListResult::Compared);
    if Source::observed(mem, &taken.entry) != Some(source) {
        SceneList::LightHandoff.count(SceneListResult::Mismatch);
    }
}

/// Read the guest-built lists once the same light reaches its constants.
pub(super) fn defer(entry: Entry, serial: u64) {
    OBSERVE.set(Some((entry, serial)));
}

pub(super) fn at_constants(esp: u32) -> Option<(Entry, u64)> {
    use engine::render::light_gather::passes::{CONSTANTS_ARGS, FRAME};
    let (entry, _) = OBSERVE.get()?;
    if entry.esp == esp.wrapping_add(4 + CONSTANTS_ARGS + FRAME) {
        OBSERVE.take()
    } else {
        None
    }
}

/// The deferred light of the passes entered at `esp`, for a caller that
/// makes no constants call (the cut frame, `super::scene_frame`).
pub(super) fn take_deferred(esp: u32) -> Option<(Entry, u64)> {
    let (entry, _) = OBSERVE.get()?;
    (entry.esp == esp).then(|| OBSERVE.take()).flatten()
}

pub(super) fn guest_made(taken: &Taken, why: Refusal) {
    super::frame::refused(SceneList::LightHandoff, why);
    defer(taken.entry, taken.serial);
}

/// A view begins: a hand-off still pending was never taken.
pub(super) fn new_view() {
    OBSERVE.set(None);
    if HANDOFF.take().is_some() {
        not_taken();
    }
}

/// A snapshot load: the pending hand-off is not this memory's.
pub(super) fn forget() {
    OBSERVE.set(None);
    HANDOFF.take();
}

/// A hand-off that no item lists' call took: its light went to the scene
/// by its passes' entry, or not at all.
fn not_taken() {
    super::frame::refused(SceneList::LightHandoff, Refusal::Layout);
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatMemory;
    use engine::render::light_gather::items::ItemRange;

    const ESP: u32 = 0x0008_0000;
    const TABLE: u32 = 0x0010_0000;
    const DATA: u32 = 0x0020_0000;
    const DEFINITION: u32 = 0xe123_0007;
    const RECEIVERS: u32 = 1 << 11 | 1 << 12 | 1 << 13 | 1 << 14;
    const CASTING: u32 = 1 << 10 | 1 << 11;

    fn passes() -> Passes {
        Passes {
            definition: DEFINITION,
            context: ESP - 0x1518 + 0x34,
            light: 0xe001_0002,
            position: 0x0030_0000 + 2 * 0x110 + 0x84,
            // The shadow byte over the visible list's argument.
            shadow: 0x0007_ab01,
            weight: 0.75f32.to_bits(),
        }
    }

    fn entry() -> Entry {
        let p = passes();
        Entry {
            eax: p.definition,
            ecx: p.context,
            esp: ESP,
            mxcsr: 0x1f80,
            x87_control: 0x027f,
        }
    }

    fn items(receivers: u32, casters: u32) -> LightItems {
        LightItems {
            kind: 1,
            records: 10..14,
            receivers: ItemRange {
                first: 0,
                count: 3,
                layers: receivers,
            },
            casters: ItemRange {
                first: 0,
                count: 2,
                layers: casters,
            },
            made: Vec::new(),
            first_person: Vec::new(),
        }
    }

    /// The passes' entry: the call's stack words under its return, the
    /// tag table, and both lists as the last light left them.
    fn guest() -> FlatMemory {
        let mut m = FlatMemory::new(0, 0x0060_0000);
        m.set_u32(guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE).unwrap();
        m.set_u32(TABLE + ((DEFINITION & 0xffff) << 4) + 8, DATA)
            .unwrap();
        m.set_u32(ESP, 0x0003_1f2b).unwrap();
        for (i, word) in passes().args().into_iter().enumerate() {
            m.set_u32(ESP + 4 + 4 * i as u32, word).unwrap();
        }
        m.set_u32(LIGHT_LIST + MASK, 1 << 14).unwrap();
        m.set_u32(CASTERS + MASK, 0).unwrap();
        m
    }

    /// The item lists as the item body leaves them.
    fn lists_made(m: &mut FlatMemory, i: &LightItems) {
        m.set_u32(LIGHT_LIST + MASK, i.receivers.layers).unwrap();
        m.set_u32(CASTERS + MASK, i.casters.layers).unwrap();
    }

    #[test]
    fn the_scene_light_from_the_hand_off_equals_the_one_read_from_memory() {
        for casters in [CASTING, 1 << 11, 0] {
            let i = items(RECEIVERS, casters);
            let mut m = guest();
            lists_made(&mut m, &i);
            let handed =
                Source::handed(&m, &passes(), i.receivers.layers, i.casters.layers).unwrap();
            let observed = Source::observed(&m, &entry()).unwrap();
            assert_eq!(handed, observed, "casters {casters:#x}");
            assert_eq!(handed.call.definition, DATA);
            assert_eq!(
                handed.call.shadow & 0xff,
                (casters >> 10) & 1,
                "the shadow byte is the casters' layer 10"
            );
            assert_eq!(
                handed.call.shadow & !0xff,
                passes().shadow & !0xff,
                "its upper bytes are kept"
            );
            // The same steps for every flag the passes can set.
            for flags in 0..16 {
                for definition_flags in [0, light_passes::DEFINITION_RECEIVERS] {
                    let a = handed.inputs(&m, flags, definition_flags);
                    let b = observed.inputs(&m, flags, definition_flags);
                    assert_eq!(a, b, "flags {flags:#x}");
                    let (Some(a), Some(b)) = (a, b) else { continue };
                    assert_eq!(
                        light_passes::steps(&a).as_slice(),
                        light_passes::steps(&b).as_slice()
                    );
                }
            }
        }
    }

    #[test]
    fn the_deferred_light_uses_its_own_completed_lists_once() {
        use engine::render::light_gather::passes::{CONSTANTS_ARGS, FRAME};
        let esp = ESP - FRAME - CONSTANTS_ARGS - 4;
        let mut m = guest();
        defer(entry(), 7);
        assert_eq!(at_constants(esp - 4), None);
        lists_made(&mut m, &items(1 << 12, 0));
        let (e, serial) = at_constants(esp).unwrap();
        assert_eq!(serial, 7);
        let source = Source::observed(&m, &e).unwrap();
        let inputs = source.inputs(&m, 0, 0).unwrap();
        assert_eq!(light_passes::steps(&inputs).drawn(), 1 << 12);
        assert_eq!(at_constants(esp), None);
    }

    #[test]
    fn at_the_passes_entry_the_lists_are_the_last_lights() {
        let i = items(1 << 12, CASTING);
        let mut m = guest();
        let handed = Source::handed(&m, &passes(), i.receivers.layers, i.casters.layers).unwrap();
        let at_entry = Source::observed(&m, &entry()).unwrap();
        assert_ne!(handed, at_entry, "read before the item lists are made");
        assert_eq!(at_entry.call.shadow & 0xff, 0, "the last casters' bit");
        assert_eq!(handed.call.shadow & 0xff, 1);
        lists_made(&mut m, &i);
        assert_eq!(Source::observed(&m, &entry()), Some(handed));
    }

    #[test]
    fn a_hand_off_is_taken_only_by_its_own_passes_item_lists() {
        let m = guest();
        // Not this call's: another context.
        hand(passes());
        let other = Entry {
            ecx: entry().ecx + 4,
            ..entry()
        };
        assert!(!entered(&m, other, 1), "the scene reads the light itself");
        assert_eq!(take(ITEMS_RETURN), None, "and the hand-off is gone");

        // No passes' entry yet: no call takes it.
        hand(passes());
        assert_eq!(take(ITEMS_RETURN), None);
        assert!(entered(&m, entry(), 2));
        assert_eq!(take(0x0003_0f00), None, "another caller's item lists");
        assert_eq!(
            take(ITEMS_RETURN),
            Some(Taken {
                passes: passes(),
                entry: entry(),
                serial: 2,
            })
        );
        assert_eq!(take(ITEMS_RETURN), None, "taken once");

        // A second entry with the hand-off still waiting is not its.
        hand(passes());
        assert!(entered(&m, entry(), 3));
        assert!(!entered(&m, entry(), 4));
        forget();
    }
}
