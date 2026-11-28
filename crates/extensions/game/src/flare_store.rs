//! The flare state in the render-private store
//! (docs/engine/sim-render-split.md, "4. The flare visibility byte",
//! slice 6). The flare state at `0x0051f408..0x005234c8` (the enable
//! bytes, the 32-byte entries at `0x0051f40c`, their count, the three
//! ended bitmaps and the frames) is written only by the frame step: the
//! render walk registers each flare (`0x000209b0`), the query pass ends
//! its test and the reader smooths its visibility byte. While a frame
//! renders, the region holds the render's own copy; when the frame ends
//! the copy is kept here, in host memory, and the simulation's bytes are
//! put back. The tick, a snapshot, a film and the arena hash then see the
//! region as the simulation left it, whatever the view drew: a headless
//! run and a drawn one hold the same bytes.
//!
//! The tick's one use of the state is `object_new` (`0x000b7b40`)
//! calling `0x0002e1e0` for the new object's index: every entry in use
//! (head bit 4) whose owner (head bits 6 to 17) is that index is freed by
//! `0x00020e50`. In the simulation's bytes no entry is ever in use, so
//! that call writes nothing. The store frees the render's entries
//! instead: it keeps each in-use entry's owner salt when a frame ends,
//! and before the next frame frees an entry whose owner slot now holds a
//! different live object, as `0x00020e50` would have.
//!
//! When the simulation's bytes change between frames, or the loaded map's
//! name does (`0x00547898`), the render's copy is dropped and the next
//! frame starts from the simulation's bytes: a new map starts with the
//! state its load left.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use guest_layout::{GuestMemory, points};
use hooks::{Builder, Feature, Manifest};

pub const ID: &str = "flare-store";
const THEATER_CAMERA: &str = "theater-camera";

/// The flare state: from the enable byte `0x0051f408` to the end of the
/// current view `0x005234c4`.
pub const BASE: u32 = 0x0051_f408;
pub const END: u32 = 0x0052_34c8;
pub const LEN: usize = (END - BASE) as usize;

const ENTRIES: u32 = 0x0051_f40c;
const ENTRY_SIZE: u32 = 32;
const COUNT: u32 = 0x0052_33ec;
/// Three bitmaps of 64 bytes: bit `i` of set `s` is entry `i`'s test.
const ENDED: u32 = 0x0052_33f0;
const ENDED_SET_BYTES: u32 = 64;
const MAX_ENTRIES: u32 = ENDED_SET_BYTES * 8;

/// `0x0002e1e0`'s test of an entry's head: in use.
const IN_USE: u32 = 0x10;

fn owner(head: u32) -> u32 {
    (head >> 6) & 0xfff
}

/// The byte offset of `addr` in the region.
fn at(addr: u32) -> usize {
    (addr - BASE) as usize
}

fn word(bytes: &[u8], addr: u32) -> u32 {
    let i = at(addr);
    u32::from_le_bytes(bytes[i..i + 4].try_into().expect("four bytes"))
}

fn set_word(bytes: &mut [u8], addr: u32, v: u32) {
    let i = at(addr);
    bytes[i..i + 4].copy_from_slice(&v.to_le_bytes());
}

/// The entry count, held to the bitmaps' capacity.
fn count(bytes: &[u8]) -> u32 {
    (word(bytes, COUNT) as i32).clamp(0, MAX_ENTRIES as i32) as u32
}

/// The in-use entries of `bytes`: index and owner.
fn in_use(bytes: &[u8]) -> impl Iterator<Item = (u32, u32)> + '_ {
    (0..count(bytes)).filter_map(move |i| {
        let head = word(bytes, ENTRIES + ENTRY_SIZE * i);
        (head & IN_USE != 0).then_some((i, owner(head)))
    })
}

/// `0x00020e50(eax index)`: the entry's bit cleared in the three ended
/// sets, the entry zeroed and `+4` set to `0xff80`.
pub fn free(bytes: &mut [u8], index: u32) {
    for set in 0..3 {
        let w = ENDED + set * ENDED_SET_BYTES + 4 * (index >> 5);
        let v = word(bytes, w);
        set_word(bytes, w, v & !(1 << (index & 31)));
    }
    let e = ENTRIES + ENTRY_SIZE * index;
    for k in 0..8 {
        set_word(bytes, e + 4 * k, if k == 1 { 0xff80 } else { 0 });
    }
}

/// The salt of object slot `index`, 0 when the slot is free, past the
/// high-water mark or the objects array is unread.
fn object_salt(mem: &dyn GuestMemory, index: u32) -> u16 {
    use guest_layout::datum::layout;
    let read = || -> Option<u16> {
        let header = mem.u32(points::OBJECTS.base).ok().filter(|&h| h != 0)?;
        let high = mem.u32(header + layout::HIGH_WATER).ok()? as i32;
        if index as i32 >= high {
            return Some(0);
        }
        let size = mem.u32(header + layout::ELEMENT_SIZE).ok()?;
        let data = mem.u32(header + layout::DATA).ok()?;
        mem.u16(data.wrapping_add(size.wrapping_mul(index))).ok()
    };
    read().unwrap_or(0)
}

/// The render's copy of the flare state and the simulation's bytes it
/// stands in for.
#[derive(Default)]
pub struct Store {
    /// The render's bytes, kept between frames.
    render: Option<Box<[u8]>>,
    /// Each in-use entry's owner slot and its salt at the last frame's end.
    owners: HashMap<u32, (u32, u16)>,
    /// The simulation's bytes put back at the last frame's end.
    left: Option<Box<[u8]>>,
    /// The simulation's bytes while a frame renders.
    restore: Option<Box<[u8]>>,
    /// The loaded map's name the render's copy was made on.
    map: [u8; MAP_NAME_LEN],
}

const MAP_NAME_LEN: usize = 32;

fn map_name(mem: &dyn GuestMemory) -> [u8; MAP_NAME_LEN] {
    let mut name = [0; MAP_NAME_LEN];
    if mem.read(guest_layout::points::CACHE_MAP_NAME.base, &mut name).is_err() {
        name = [0; MAP_NAME_LEN];
    }
    name
}

impl Store {
    /// Puts the render's bytes in the region, keeping the simulation's
    /// for [`Store::leave`]. The render's copy starts from the
    /// simulation's bytes on the first frame and whenever the simulation
    /// changed them since the last frame. Entered already: nothing.
    pub fn enter(&mut self, mem: &mut dyn GuestMemory) {
        if self.restore.is_some() {
            return;
        }
        let mut sim = vec![0; LEN].into_boxed_slice();
        if mem.read(BASE, &mut sim).is_err() {
            return;
        }
        let map = map_name(&*mem);
        if self.left.as_deref() != Some(&sim[..]) || map != self.map {
            self.render = None;
            self.owners.clear();
            self.map = map;
        }
        let mut render = self.render.take().unwrap_or_else(|| sim.clone());
        self.free_reused(&*mem, &mut render);
        if mem.write(BASE, &render).is_ok() {
            self.restore = Some(sim);
        }
    }

    /// `object_new`'s free, for the render's entries: an entry whose
    /// owner slot holds a live object other than the one it held when the
    /// last frame ended is freed.
    fn free_reused(&mut self, mem: &dyn GuestMemory, render: &mut [u8]) {
        let reused: Vec<u32> = in_use(render)
            .filter(|&(i, o)| {
                let now = object_salt(mem, o);
                let then = self.owners.get(&i).filter(|&&(slot, _)| slot == o).map_or(0, |&(_, s)| s);
                now != 0 && now != then
            })
            .map(|(i, _)| i)
            .collect();
        for i in reused {
            free(render, i);
        }
    }

    /// Keeps the render's bytes and each in-use entry's owner salt, and
    /// puts the simulation's bytes back. Not entered: nothing.
    pub fn leave(&mut self, mem: &mut dyn GuestMemory) {
        let Some(sim) = self.restore.take() else {
            return;
        };
        let mut render = vec![0; LEN].into_boxed_slice();
        if mem.read(BASE, &mut render).is_ok() {
            self.owners = in_use(&render).map(|(i, o)| (i, (o, object_salt(&*mem, o)))).collect();
            self.render = Some(render);
        } else {
            self.render = None;
            self.owners.clear();
        }
        if mem.write(BASE, &sim).is_ok() {
            self.left = Some(sim);
        } else {
            self.left = None;
        }
    }

    /// The render's copy, between frames.
    pub fn render(&self) -> Option<&[u8]> {
        self.render.as_deref()
    }
}

fn shared() -> Arc<Mutex<Store>> {
    static SHARED: OnceLock<Arc<Mutex<Store>>> = OnceLock::new();
    SHARED.get_or_init(Default::default).clone()
}

/// The flare state held by the render for every frame. Registered by
/// boot for every session that renders; a headless host has no render,
/// so its region is the simulation's alone.
pub struct FlareStore;

impl Feature for FlareStore {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The flare state in the render-private store").doc("docs/engine/sim-render-split.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
        // A snapshot is taken between frames, when the region holds the
        // simulation's bytes; the render's copy only ever reaches drawing.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let store = shared();
        let entering = store.clone();
        b.raw()
            .before(&points::RENDER_FRAME, "enter the render's flare state", move |call| {
                entering.lock().unwrap_or_else(|e| e.into_inner()).enter(&mut *call.ctx.memory());
                Ok(hooks::Flow::Continue)
            })
            .after_feature(crate::bot_glue::ID)
            .after_feature(THEATER_CAMERA)
            .after_feature(crate::render_rng::ID);
        b.raw()
            .after(&points::RENDER_FRAME, "restore the simulation's flare state", move |call| {
                store.lock().unwrap_or_else(|e| e.into_inner()).leave(&mut *call.ctx.memory());
                Ok(())
            })
            .after_feature(crate::bot_glue::ID)
            .after_feature(THEATER_CAMERA)
            .after_feature(crate::render_rng::ID);
        Ok(())
    }
}

pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry.add(&FlareStore).map_err(|e| anyhow::anyhow!("flare store: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const OBJECTS: u32 = 0x0060_0000;
    const DATA: u32 = 0x0060_1000;
    const OBJECT_SIZE: u32 = 0xc;

    fn guest() -> FakeGuest {
        use guest_layout::datum::layout;
        let mut m = FakeGuest::new();
        m.put(BASE, &vec![0; LEN]);
        m.put_u8(BASE, 1).put_u8(BASE + 1, 1);
        m.put_u32(points::OBJECTS.base, OBJECTS)
            .put_u32(OBJECTS + layout::HIGH_WATER, 8)
            .put_u32(OBJECTS + layout::ELEMENT_SIZE, OBJECT_SIZE)
            .put_u32(OBJECTS + layout::DATA, DATA)
            .put(DATA, &[0; 8 * OBJECT_SIZE as usize]);
        m
    }

    fn region(m: &FakeGuest) -> Vec<u8> {
        let mut out = vec![0; LEN];
        m.read(BASE, &mut out).unwrap();
        out
    }

    fn set_salt(m: &mut FakeGuest, index: u32, salt: u16) {
        m.put_u16(DATA + OBJECT_SIZE * index, salt);
    }

    /// What the render does in a frame: registers a flare owned by
    /// object `owner` in entry 0, ends its test in set 1 and gives it a
    /// visibility byte.
    fn draw(m: &mut FakeGuest, owner: u32, byte: u8) {
        let head = IN_USE | 2 << 1 | owner << 6 | u32::from(byte) << 24;
        m.put_u32(ENTRIES, head).put_u32(ENTRIES + 4, 2 | 16 << 16);
        m.put_u32(COUNT, 1);
        m.put_u32(ENDED + ENDED_SET_BYTES, 1);
    }

    /// The rule for the flare state: a frame leaves the simulation's
    /// bytes as they were, whether the flare drew fully visible or fully
    /// hidden, and the next frame sees the render's own.
    #[test]
    fn a_frame_leaves_the_flare_state_as_the_simulation_left_it() {
        for byte in [255u8, 0] {
            let mut m = guest();
            let before = region(&m);
            let mut store = Store::default();
            store.enter(&mut m);
            draw(&mut m, 3, byte);
            store.leave(&mut m);
            assert_eq!(region(&m), before, "byte {byte}");
            store.enter(&mut m);
            assert_eq!(m.u32(ENTRIES).unwrap() >> 24, u32::from(byte), "the render's entry is back");
            assert_eq!(m.u32(COUNT).unwrap(), 1);
            store.leave(&mut m);
            assert_eq!(region(&m), before);
        }
    }

    /// A new map starts from the state its load left, even when the
    /// load wrote the bytes the region already held.
    #[test]
    fn a_new_map_drops_the_renders_copy() {
        let mut m = guest();
        m.put(points::CACHE_MAP_NAME.base, b"lockout\0");
        let before = region(&m);
        let mut store = Store::default();
        store.enter(&mut m);
        draw(&mut m, 3, 9);
        store.leave(&mut m);
        m.put(points::CACHE_MAP_NAME.base, b"zanzibar\0");
        store.enter(&mut m);
        assert_eq!(region(&m), before);
        store.leave(&mut m);
    }

    #[test]
    fn a_second_enter_or_leave_in_a_frame_does_nothing() {
        let mut m = guest();
        let before = region(&m);
        let mut store = Store::default();
        store.enter(&mut m);
        draw(&mut m, 3, 9);
        store.enter(&mut m);
        assert_eq!(m.u32(COUNT).unwrap(), 1, "a second enter keeps the frame's writes");
        store.leave(&mut m);
        store.leave(&mut m);
        assert_eq!(region(&m), before);
        assert_eq!(store.render().map(|r| word(r, COUNT)), Some(1));
    }

    /// A map load (or a revert) changes the simulation's bytes: the
    /// render's copy is dropped and the next frame starts from them.
    #[test]
    fn the_simulations_change_drops_the_renders_copy() {
        let mut m = guest();
        let mut store = Store::default();
        store.enter(&mut m);
        draw(&mut m, 3, 9);
        store.leave(&mut m);
        m.put_u8(BASE + 1, 0); // the next map's state
        let loaded = region(&m);
        store.enter(&mut m);
        assert_eq!(region(&m), loaded, "the render starts from the simulation's bytes");
        store.leave(&mut m);
        assert_eq!(region(&m), loaded);
    }

    /// `object_new`'s free of a reused index (`0x0002e1e0`), made on the
    /// render's copy: the entry of an owner slot that now holds another
    /// object is freed as `0x00020e50` frees it; one whose owner is
    /// unchanged, or only deleted, keeps its state.
    #[test]
    fn a_reused_owner_slot_frees_its_flares() {
        let mut m = guest();
        set_salt(&mut m, 3, 0xe001);
        let mut store = Store::default();
        store.enter(&mut m);
        draw(&mut m, 3, 200);
        store.leave(&mut m);

        store.enter(&mut m);
        assert_eq!(m.u32(ENTRIES).unwrap() >> 24, 200, "the same owner: kept");
        store.leave(&mut m);

        set_salt(&mut m, 3, 0); // deleted
        store.enter(&mut m);
        assert_eq!(m.u32(ENTRIES).unwrap() >> 24, 200, "a deleted owner: kept, as retail keeps it");
        store.leave(&mut m);

        set_salt(&mut m, 3, 0xe002); // a new object in the slot
        let sim = region(&m);
        store.enter(&mut m);
        assert_eq!(m.u32(ENTRIES).unwrap(), 0, "freed");
        assert_eq!(m.u32(ENTRIES + 4).unwrap(), 0xff80);
        assert_eq!(m.u32(ENTRIES + 8).unwrap(), 0);
        assert_eq!(m.u32(ENDED + ENDED_SET_BYTES).unwrap() & 1, 0, "its ended bit cleared");
        store.leave(&mut m);
        assert_eq!(region(&m), sim, "the tick's bytes untouched");
    }

    /// An entry registered while its owner slot was empty is freed when an
    /// object is made there, as `object_new` frees it.
    #[test]
    fn an_object_made_in_an_empty_owner_slot_frees_its_flares() {
        let mut m = guest();
        let mut store = Store::default();
        store.enter(&mut m);
        draw(&mut m, 5, 40);
        store.leave(&mut m);
        set_salt(&mut m, 5, 0xe010);
        store.enter(&mut m);
        assert_eq!(m.u32(ENTRIES).unwrap(), 0);
        store.leave(&mut m);
    }

    #[test]
    fn free_matches_the_guests_free() {
        let mut bytes = vec![0xffu8; LEN];
        set_word(&mut bytes, COUNT, 40);
        free(&mut bytes, 33);
        let e = ENTRIES + ENTRY_SIZE * 33;
        let words: Vec<u32> = (0..8).map(|k| word(&bytes, e + 4 * k)).collect();
        assert_eq!(words, [0, 0xff80, 0, 0, 0, 0, 0, 0]);
        for set in 0..3 {
            assert_eq!(word(&bytes, ENDED + set * ENDED_SET_BYTES + 4), !2, "set {set}");
            assert_eq!(word(&bytes, ENDED + set * ENDED_SET_BYTES), u32::MAX);
        }
        assert_eq!(word(&bytes, e + 32), u32::MAX, "the next entry untouched");
    }
}
