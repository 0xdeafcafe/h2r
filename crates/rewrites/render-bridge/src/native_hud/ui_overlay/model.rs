//! Model widget selection at the sorted draw. See the dated model-widget note.
use scene_extract::{
    GuestRead,
    ui::{UiContent, Unread},
};

const SCENARIO: u32 = 0x004e_0350;
const OBJECT_HANDLES: u32 = 0x004d_e2d0;

fn word(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Unread> {
    base.checked_add(offset)
        .and_then(|p| mem.read_u32(p))
        .ok_or(Unread::Unreadable)
}

fn pointer(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Unread> {
    let ptr = word(mem, base, offset)?;
    (ptr != 0).then_some(ptr).ok_or(Unread::Unreadable)
}

// Widget names occupy 32 bytes; scenario names occupy the first 32 of 36.
fn name(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<Vec<u8>, Unread> {
    let p = base.checked_add(offset).ok_or(Unread::Unreadable)?;
    let mut bytes = Vec::new();
    for at in (0..32).step_by(4) {
        for byte in word(mem, p, at)?.to_le_bytes() {
            if byte == 0 {
                return Ok(bytes);
            }
            bytes.push(byte);
        }
    }
    Err(Unread::Model)
}

/// The inactive selector and an empty object selection produce no view.
/// A resolved object needs the guest's object update and nested scene.
pub(super) fn read(mem: &dyn GuestRead, widget: u32) -> Result<UiContent, Unread> {
    let selection =
        engine::ui_model::selection(widget, |at| mem.read_u32(at)).ok_or(Unread::Unreadable)?;
    let engine::ui_model::Selection::Names {
        definition: def,
        objects: count,
        lights,
    } = selection
    else {
        return Ok(UiContent::None);
    };
    if lights > 0 || count > 32 {
        return Err(Unread::Model);
    }
    if count <= 0 {
        return Ok(UiContent::None);
    }
    let names = pointer(mem, def, 0x10)?;
    for index in 0..count as u32 {
        let key = name(mem, names, index * 32)?;
        if key.is_empty() {
            continue;
        }
        let scenario = pointer(mem, SCENARIO, 0)?;
        let rows = word(mem, scenario, 0x48)? as i32;
        if rows > i32::from(i16::MAX) {
            return Err(Unread::Model);
        }
        if rows <= 0 {
            continue;
        }
        let table = pointer(mem, scenario, 0x4c)?;
        // The resolver rejects indices >= 640. Only the first equal name
        // counts, including when its handle is the absent datum.
        for row in 0..rows.min(640) as u32 {
            if name(mem, table, row * 36)? == key {
                let handles = pointer(mem, OBJECT_HANDLES, 0)?;
                if word(mem, handles, row * 4)? != u32::MAX {
                    return Err(Unread::Model);
                }
                break;
            }
        }
    }
    Ok(UiContent::None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Words(HashMap<u32, u32>);
    impl GuestRead for Words {
        fn read_u32(&self, address: u32) -> Option<u32> {
            self.0.get(&address).copied()
        }
    }

    #[test]
    fn first_matching_name_selects_its_live_handle() {
        let mut mem = Words(HashMap::from([
            (0x0054_d598, 4),
            (0x1070, 0x2000),
            (0x200c, 1),
            (0x2010, 0x3000),
            (0x2014, 0),
            (0x3000, 65),
            (SCENARIO, 0x4000),
            (0x4048, 2),
            (0x404c, 0x5000),
            (0x5000, 65),
            (0x5024, 65),
            (OBJECT_HANDLES, 0x6000),
            (0x6000, u32::MAX),
            (0x6004, 9),
        ]));
        assert!(matches!(read(&mem, 0x1000), Ok(UiContent::None)));
        mem.0.insert(0x6000, 42);
        assert!(matches!(read(&mem, 0x1000), Err(Unread::Model)));
        mem.0.remove(&0x6000);
        assert!(matches!(read(&mem, 0x1000), Err(Unread::Unreadable)));
    }
}
