//! The HUD's walks of its definitions in Rust (docs/graphics/native-hud-overlay.md,
//! "The definitions in Rust"): at `HUD_DEFINITIONS` (`0x001394b9`) the
//! definitions the HUD record names are walked in the guest's order
//! (`hud_kit_core::walk::definitions`), each by Rust's own walk
//! ([`super::walk`]), and the guest's body is left out.
//!
//! A record that does not read panics, named.

use guest_layout::memory::GuestMemory;
use scene_extract::hud;
use scene_extract::hud::walk::DefinitionWalk;

/// The UI globals and the HUD record's place in them, which the body
/// writes before its first walk (`0x001394bf`).
pub const UI: u32 = 0x0051_0c4c;
pub const UI_RECORD: u32 = 0x1bc;

/// The walks the HUD record at `record` names, in the guest's order.
pub fn walks(mem: &dyn GuestMemory, record: u32) -> Vec<DefinitionWalk> {
    let Some(walks) = hud::record(mem, record).and_then(|r| hud::walk::definitions(&r)) else {
        unimplemented!("hud definitions: the record at {record:#x} did not read");
    };
    walks
}

/// The body's first write (`0x001394bf`): the HUD record into the UI
/// globals. `Err` names what did not read or take it.
pub fn bind(mem: &mut dyn GuestMemory, record: u32) -> Result<(), &'static str> {
    let ui = mem.u32(UI).ok().filter(|&ui| ui != 0).ok_or("the UI globals")?;
    let at = ui.checked_add(UI_RECORD).ok_or("the UI record's place")?;
    mem.write(at, &record.to_le_bytes()).map_err(|_| "the UI record's place")
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_record_is_written_into_the_ui_globals() {
        let mut m = FakeGuest::new();
        assert_eq!(bind(&mut m, 0x7000), Err("the UI globals"));
        m.put_u32(UI, 0x8000);
        m.map(0x8000 + UI_RECORD, 4);
        assert_eq!(bind(&mut m, 0x7000), Ok(()));
        assert_eq!(m.u32(0x8000 + UI_RECORD).unwrap(), 0x7000);
    }
}
