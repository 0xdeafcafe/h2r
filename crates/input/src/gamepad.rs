//! The Xbox gamepad exactly as titles see it through XAPI's XInput.
//!
//! Sources (all pinned to a commit):
//!
//! * `XINPUT_GAMEPAD`, `XINPUT_STATE`, `XINPUT_CAPABILITIES`, `XINPUT_RUMBLE`,
//!   `XINPUT_FEEDBACK`, `XINPUT_POLLING_PARAMETERS`, `XINPUT_DEVTYPE_*`,
//!   `XINPUT_DEVSUBTYPE_*`: Cxbx-Reloaded `Xapi.h`
//!   <https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.h>
//! * Button bit positions and analog-button indices (A, B, X, Y, Black,
//!   White, left trigger, right trigger; d-pad = bit 0..3, Start, Back, thumb
//!   clicks = bits 4..7): xemu's XID report definitions, where
//!   `BUTTON_MASK(b) = 1 << (b - GAMEPAD_DPAD_UP)`
//!   <https://github.com/xemu-project/xemu/blob/4bdad199102b22ed0cfa998e9af6b35d76238b3e/hw/xbox/xid.h>
//! * Packet number semantics (incremented when new input is read, copied into
//!   `XINPUT_STATE`) and `ERROR_*` returns: Cxbx-Reloaded `Xapi.cpp`
//!   <https://github.com/Cxbx-Reloaded/Cxbx-Reloaded/blob/585c49a50af1255ab155099e06f24505f9c5a800/src/core/hle/XAPI/Xapi.cpp>
//!
//! The Win32 error values (`ERROR_SUCCESS` 0, `ERROR_IO_PENDING` 997,
//! `ERROR_DEVICE_NOT_CONNECTED` 1167) are the standard winerror.h numbers.

/// `ERROR_SUCCESS`.
pub const ERROR_SUCCESS: u32 = 0;
/// `ERROR_IO_PENDING`: `XInputSetState` queued the feedback.
pub const ERROR_IO_PENDING: u32 = 997;
/// `ERROR_DEVICE_NOT_CONNECTED`.
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

/// `XINPUT_DEVTYPE_GAMEPAD`.
pub const XINPUT_DEVTYPE_GAMEPAD: u8 = 0x01;
/// `XINPUT_DEVSUBTYPE_GC_GAMEPAD` (standard controller).
pub const XINPUT_DEVSUBTYPE_GC_GAMEPAD: u8 = 0x01;

/// The 16-bit `wButtons` digital field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Buttons(pub u16);

impl Buttons {
    pub const DPAD_UP: Buttons = Buttons(0x0001);
    pub const DPAD_DOWN: Buttons = Buttons(0x0002);
    pub const DPAD_LEFT: Buttons = Buttons(0x0004);
    pub const DPAD_RIGHT: Buttons = Buttons(0x0008);
    pub const START: Buttons = Buttons(0x0010);
    pub const BACK: Buttons = Buttons(0x0020);
    pub const LEFT_THUMB: Buttons = Buttons(0x0040);
    pub const RIGHT_THUMB: Buttons = Buttons(0x0080);

    pub const fn contains(self, other: Buttons) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn set(&mut self, other: Buttons, down: bool) {
        if down {
            self.0 |= other.0;
        } else {
            self.0 &= !other.0;
        }
    }
}

impl std::ops::BitOr for Buttons {
    type Output = Buttons;
    fn bitor(self, rhs: Buttons) -> Buttons {
        Buttons(self.0 | rhs.0)
    }
}

/// Index into `bAnalogButtons[8]`. Each is a 0..=255 pressure value on the
/// Xbox controller (the face buttons are pressure sensitive too).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AnalogButton {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    Black = 4,
    White = 5,
    LeftTrigger = 6,
    RightTrigger = 7,
}

impl AnalogButton {
    pub const ALL: [AnalogButton; 8] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::Black,
        Self::White,
        Self::LeftTrigger,
        Self::RightTrigger,
    ];
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// `XINPUT_GAMEPAD` (packed, 18 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gamepad {
    pub buttons: Buttons,
    pub analog: [u8; 8],
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

impl Gamepad {
    /// Size of the guest struct (`#pragma pack(1)`).
    pub const SIZE: usize = 18;

    pub fn analog(&self, b: AnalogButton) -> u8 {
        self.analog[b.index()]
    }

    /// Serialise to the guest's little-endian packed layout, for writing into
    /// guest memory.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut o = [0u8; Self::SIZE];
        o[0..2].copy_from_slice(&self.buttons.0.to_le_bytes());
        o[2..10].copy_from_slice(&self.analog);
        o[10..12].copy_from_slice(&self.thumb_lx.to_le_bytes());
        o[12..14].copy_from_slice(&self.thumb_ly.to_le_bytes());
        o[14..16].copy_from_slice(&self.thumb_rx.to_le_bytes());
        o[16..18].copy_from_slice(&self.thumb_ry.to_le_bytes());
        o
    }

    pub fn from_bytes(b: &[u8; Self::SIZE]) -> Self {
        let i16_at = |i: usize| i16::from_le_bytes([b[i], b[i + 1]]);
        Gamepad {
            buttons: Buttons(u16::from_le_bytes([b[0], b[1]])),
            analog: b[2..10].try_into().expect("8 bytes"),
            thumb_lx: i16_at(10),
            thumb_ly: i16_at(12),
            thumb_rx: i16_at(14),
            thumb_ry: i16_at(16),
        }
    }
}

/// `XINPUT_STATE`: packet number plus gamepad (22 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub packet_number: u32,
    pub gamepad: Gamepad,
}

impl State {
    pub const SIZE: usize = 4 + Gamepad::SIZE;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut o = [0u8; Self::SIZE];
        o[0..4].copy_from_slice(&self.packet_number.to_le_bytes());
        o[4..].copy_from_slice(&self.gamepad.to_bytes());
        o
    }
}

/// `XINPUT_RUMBLE`: motor speeds, 0..=65535.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rumble {
    pub left_motor: u16,
    pub right_motor: u16,
}

impl Rumble {
    pub const SIZE: usize = 4;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let l = self.left_motor.to_le_bytes();
        let r = self.right_motor.to_le_bytes();
        [l[0], l[1], r[0], r[1]]
    }

    pub fn from_bytes(b: &[u8; Self::SIZE]) -> Self {
        Rumble { left_motor: u16::from_le_bytes([b[0], b[1]]), right_motor: u16::from_le_bytes([b[2], b[3]]) }
    }
}

/// `XINPUT_FEEDBACK` as laid out by Cxbx-Reloaded `Xapi.h` (`#pragma pack(1)`,
/// 32-bit guest): `XINPUT_FEEDBACK_HEADER` is `dwStatus` u32 @0, `hEvent`
/// HANDLE @4, `Unknown1[4]` @8, `IoCompletedEvent` PVOID @12, `Unknown2[48]`
/// @16, `bReportId` @64, `bLength` @65 (66 bytes), then `XINPUT_RUMBLE` @66.
/// Total 70 bytes. The unknown/reserved header fields are runtime-private and
/// not modelled; `to_bytes` writes them as zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Feedback {
    /// `Header.dwStatus`: `ERROR_IO_PENDING` while queued, `ERROR_SUCCESS`
    /// once the device took it.
    pub status: u32,
    /// `Header.hEvent`: guest handle to signal on completion (0 = none).
    pub h_event: u32,
    pub rumble: Rumble,
}

impl Feedback {
    pub const HEADER_SIZE: usize = 66;
    pub const OFFSET_STATUS: usize = 0;
    pub const OFFSET_H_EVENT: usize = 4;
    pub const OFFSET_RUMBLE: usize = Self::HEADER_SIZE;
    pub const SIZE: usize = Self::HEADER_SIZE + Rumble::SIZE;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut o = [0u8; Self::SIZE];
        o[0..4].copy_from_slice(&self.status.to_le_bytes());
        o[4..8].copy_from_slice(&self.h_event.to_le_bytes());
        o[Self::OFFSET_RUMBLE..].copy_from_slice(&self.rumble.to_bytes());
        o
    }

    pub fn from_bytes(b: &[u8; Self::SIZE]) -> Self {
        Feedback {
            status: u32::from_le_bytes(b[0..4].try_into().expect("4 bytes")),
            h_event: u32::from_le_bytes(b[4..8].try_into().expect("4 bytes")),
            rumble: Rumble::from_bytes(b[Self::OFFSET_RUMBLE..].try_into().expect("4 bytes")),
        }
    }
}

/// `XINPUT_CAPABILITIES` (packed; Cxbx-Reloaded brackets it with
/// `AlignPrefix1.h`/`AlignPosfix1.h`): `SubType` u8 @0, `Reserved` u16 @1,
/// `In` (`XINPUT_GAMEPAD`) @3, `Out` (`XINPUT_RUMBLE`) @21, 25 bytes. `In` has
/// a bit/value set for every input the device can report, `Out` the rumble
/// motors it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub sub_type: u8,
    pub input: Gamepad,
    pub output: Rumble,
}

impl Capabilities {
    pub const SIZE: usize = 3 + Gamepad::SIZE + Rumble::SIZE;
    pub const OFFSET_IN: usize = 3;
    pub const OFFSET_OUT: usize = 3 + Gamepad::SIZE;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut o = [0u8; Self::SIZE];
        o[0] = self.sub_type;
        // o[1..3] is the reserved word, zero.
        o[Self::OFFSET_IN..Self::OFFSET_OUT].copy_from_slice(&self.input.to_bytes());
        o[Self::OFFSET_OUT..].copy_from_slice(&self.output.to_bytes());
        o
    }

    pub fn from_bytes(b: &[u8; Self::SIZE]) -> Self {
        Capabilities {
            sub_type: b[0],
            input: Gamepad::from_bytes(b[Self::OFFSET_IN..Self::OFFSET_OUT].try_into().expect("18 bytes")),
            output: Rumble::from_bytes(b[Self::OFFSET_OUT..].try_into().expect("4 bytes")),
        }
    }
    /// The standard controller: everything present.
    pub fn standard() -> Self {
        Capabilities {
            sub_type: XINPUT_DEVSUBTYPE_GC_GAMEPAD,
            input: Gamepad {
                buttons: Buttons(0x00FF),
                analog: [0xFF; 8],
                thumb_lx: -1,
                thumb_ly: -1,
                thumb_rx: -1,
                thumb_ry: -1,
            },
            output: Rumble { left_motor: 0xFFFF, right_motor: 0xFFFF },
        }
    }
}

/// `XINPUT_POLLING_PARAMETERS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollingParameters {
    /// `fAutoPoll`: the runtime refreshes state on its own. Default on.
    pub auto_poll: bool,
    pub interrupt_out: bool,
    pub input_interval: u8,
    pub output_interval: u8,
}

impl Default for PollingParameters {
    fn default() -> Self {
        PollingParameters { auto_poll: true, interrupt_out: false, input_interval: 8, output_interval: 8 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_masks_match_xid_layout() {
        // xemu: BUTTON_MASK(b) = 1 << (b - GAMEPAD_DPAD_UP)
        assert_eq!(Buttons::DPAD_UP.0, 1 << 0);
        assert_eq!(Buttons::DPAD_RIGHT.0, 1 << 3);
        assert_eq!(Buttons::START.0, 1 << 4);
        assert_eq!(Buttons::BACK.0, 1 << 5);
        assert_eq!(Buttons::LEFT_THUMB.0, 1 << 6);
        assert_eq!(Buttons::RIGHT_THUMB.0, 1 << 7);
    }

    #[test]
    fn bytes_roundtrip_and_layout() {
        let g = Gamepad {
            buttons: Buttons::START | Buttons::DPAD_UP,
            analog: [1, 2, 3, 4, 5, 6, 7, 8],
            thumb_lx: -2,
            thumb_ly: 0x1234,
            thumb_rx: i16::MIN,
            thumb_ry: i16::MAX,
        };
        let b = g.to_bytes();
        assert_eq!(&b[0..2], &[0x11, 0x00]);
        assert_eq!(&b[2..10], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&b[10..12], &[0xFE, 0xFF]);
        assert_eq!(&b[12..14], &[0x34, 0x12]);
        assert_eq!(Gamepad::from_bytes(&b), g);
        assert_eq!(State::SIZE, 22);
    }

    #[test]
    fn feedback_guest_layout() {
        assert_eq!(Feedback::SIZE, 70);
        let f = Feedback {
            status: ERROR_IO_PENDING,
            h_event: 0xAABBCCDD,
            rumble: Rumble { left_motor: 0x1234, right_motor: 0xFEDC },
        };
        let b = f.to_bytes();
        assert_eq!(&b[0..4], &997u32.to_le_bytes());
        assert_eq!(&b[4..8], &[0xDD, 0xCC, 0xBB, 0xAA]);
        assert!(b[8..66].iter().all(|&x| x == 0));
        assert_eq!(&b[66..70], &[0x34, 0x12, 0xDC, 0xFE]);
        assert_eq!(Feedback::from_bytes(&b), f);
    }

    #[test]
    fn capabilities_guest_layout() {
        assert_eq!(Capabilities::SIZE, 25);
        let c = Capabilities::standard();
        let b = c.to_bytes();
        assert_eq!(b[0], 1);
        assert_eq!(&b[1..3], &[0, 0]);
        assert_eq!(&b[3..21], &c.input.to_bytes());
        assert_eq!(&b[3..5], &[0xFF, 0x00]); // wButtons @3
        assert_eq!(&b[5..13], &[0xFF; 8]); // bAnalogButtons @5
        assert_eq!(&b[21..25], &[0xFF; 4]); // Out rumble
        assert_eq!(Capabilities::from_bytes(&b), c);
    }

    #[test]
    fn analog_indices() {
        assert_eq!(AnalogButton::RightTrigger.index(), 7);
        assert_eq!(AnalogButton::Black.index(), 4);
    }
}
