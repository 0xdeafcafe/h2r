//! Whether the HUD screen pass is drawing a converted scope-zoom profile
//! (an `egor` tag in the `tags::scope_zoom` format, from a Halo 1 map).
//! `native_post` leaves the screen effect to the original when it is, and
//! the Halo 1 scope-zoom hook reads the same profiles.

use engine::handle::DatumHandle;
use engine::memory::GuestMemory;
use engine::tags::{entry_layout, tag_entry_checked};
use tags::scope_zoom::{MAGIC, Profile, WORDS};

/// The return address of the HUD screen pass's call to the screen effect.
pub const HUD_SCREEN_RETURN: u32 = 0x0022_bbc0;

/// A tag's definition address and size, when the datum is a tag of
/// `group` with a definition.
pub fn tag_def(mem: &dyn GuestMemory, tag: u32, group: [u8; 4]) -> Option<(u32, u32)> {
    let entry = tag_entry_checked(mem, DatumHandle::from_raw(tag)?).ok()??;
    if mem.u32(entry + entry_layout::GROUP).ok()? != u32::from_be_bytes(group) {
        return None;
    }
    let def = mem.u32(entry + entry_layout::ADDRESS).ok()?;
    let size = mem.u32(entry + entry_layout::SIZE).ok()?;
    (def != 0).then_some((def, size))
}

/// The scope-zoom profile an `egor` tag holds, if it holds one.
pub fn profile(mem: &dyn GuestMemory, datum: u32) -> Option<Profile> {
    let (def, size) = tag_def(mem, datum, *b"egor")?;
    if size < (WORDS * 4) as u32 || mem.u32(def).ok()? != MAGIC {
        return None;
    }
    let mut bytes = [0; WORDS * 4];
    mem.read(def, &mut bytes).ok()?;
    Profile::decode(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
    }))
}

/// The profile of the screen effect the HUD screen pass is calling, if
/// the call is that pass's (`esp` holds [`HUD_SCREEN_RETURN`]).
pub fn hud_profile(mem: &dyn GuestMemory, datum: u32, esp: u32) -> Option<Profile> {
    if mem.u32(esp).ok()? != HUD_SCREEN_RETURN {
        return None;
    }
    profile(mem, datum)
}
