//! Object-bound loop identities and the game sound record's playback policy.
//! A definition can group several records by tag or by their owning object.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub record: u32,
    pub tag: u32,
    pub kind: u8,
    pub key: i8,
}

impl Binding {
    pub fn playback_key(self) -> u32 {
        if self.kind == 0 && self.key != -1 {
            self.tag | 0x8000
        } else {
            self.record
        }
    }

    /// The object pointer is only read by the guest when the definition
    /// requests an object group and this record is attached to one.
    pub fn group(self, definition: u32, object: impl FnOnce() -> u32) -> Option<u32> {
        let attached = self.kind == 1;
        if definition & 0x20 == 0 && (!attached || definition & 0x10 == 0) {
            return None;
        }
        if attached {
            let owner = object();
            if owner != u32::MAX {
                return Some(owner);
            }
        }
        Some(self.record | 0x2000)
    }
}

/// Flags passed to the loop player. Kinds 1 through 3 require a spatial
/// validation unless the definition explicitly disables it.
pub fn playback_flags(definition: u32, kind: u8) -> u32 {
    let check = (1u32 << (kind & 31)) & 14 != 0 && definition & 0x40 == 0;
    u32::from(definition & 8 == 0) | (u32::from(check) << 1)
}

pub fn track_flags(flags: u16, kind: u8) -> u32 {
    u32::from(flags & 0x10 != 0)
        | (u32::from(flags & 0x80 != 0) << 2)
        | (u32::from(kind == 4) << 4)
}

pub fn should_release(stage: u8, flags: u8) -> bool {
    stage == 3 && flags & 1 != 0 && flags & 14 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_groups_and_objectless_fallback() {
        let attached = Binding { record: 0x12340005, tag: 42, kind: 1, key: 8 };
        assert_eq!(attached.playback_key(), attached.record);
        assert_eq!(attached.group(0x10, || 77), Some(77));
        assert_eq!(attached.group(0x20, || u32::MAX), Some(attached.record | 0x2000));
        let plain = Binding { kind: 0, ..attached };
        assert_eq!(plain.playback_key(), 42 | 0x8000);
        assert_eq!(plain.group(0x10, || unreachable!()), None);
        assert_eq!(plain.group(0x20, || unreachable!()), Some(plain.record | 0x2000));
        assert_eq!(Binding { key: -1, ..plain }.playback_key(), plain.record);
    }

    #[test]
    fn playback_and_release_flags_are_independent() {
        assert_eq!(playback_flags(0, 2), 3);
        assert_eq!(playback_flags(0x48, 2), 0);
        assert_eq!(playback_flags(0x08, 4), 0);
        assert_eq!(track_flags(0x90, 4), 21);
        assert!(should_release(3, 5));
        assert!(!should_release(2, 5));
        assert!(!should_release(3, 4));
        assert!(!should_release(3, 1));
    }
}
