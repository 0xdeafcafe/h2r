//! Steam Controller HID reports: which devices are Steam Controllers, their
//! input reports parsed into one [`PadState`], and the feature and output
//! reports that turn lizard mode off and on and drive the trackpad haptics.
//!
//! Two controllers carry the name:
//!
//! * the original Steam Controller (2015, "D0G"): wired (`0x1102`), through
//!   its wireless dongle (`0x1142`, four controller slots) and over Bluetooth
//!   LE with the BLE firmware (`0x1105`, `0x1106`);
//! * the Steam Controller (2026, "Triton"/"Ibex" in the sources): wired
//!   (`0x1302`), Bluetooth LE (`0x1303`) and through its wireless puck or a
//!   Steam Machine's receiver (`0x1304`, `0x1305`).
//!
//! Everything here is byte-level and follows two public drivers, pinned in
//! `docs/steam-controller.md`: SDL's `SDL_hidapi_steam.c`,
//! `SDL_hidapi_steam_triton.c` and `steam/controller_structs.h`, and the
//! Linux kernel's `drivers/hid/hid-steam.c`. Opening the device is the host's
//! business (`crate::steam_pad`).
//!
//! Report bytes are as the host delivers them: the report ID first when the
//! device numbers its reports. The original controller's wired and dongle
//! interfaces do not (their input reports start with the
//! `ValveInReportHeader_t`, and feature reports go out as report 0); its
//! Bluetooth interface sends report `0x03` segments; the 2026 controller
//! numbers everything.

use std::time::Duration;

/// Valve's USB vendor ID.
pub const VALVE_VID: u16 = 0x28de;

/// Which Steam Controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    /// The 2015 controller: one stick, two clickable trackpads.
    Original,
    /// The 2026 controller: two sticks, a d-pad and two trackpads.
    Triton,
}

/// How it is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Link {
    Wired,
    /// A wireless receiver: each controller slot is an interface of its own.
    Dongle,
    Bluetooth,
}

/// Every product ID handled, from SDL's `controller_list.h` and Linux's
/// `hid-ids.h`. The Steam Deck (`0x1205`) and the 2015 "HEADCRAB"
/// prototypes are left out.
pub const PRODUCTS: &[(u16, Model, Link)] = &[
    (0x1102, Model::Original, Link::Wired),
    (0x1142, Model::Original, Link::Dongle),
    (0x1105, Model::Original, Link::Bluetooth),
    (0x1106, Model::Original, Link::Bluetooth),
    (0x1302, Model::Triton, Link::Wired),
    (0x1303, Model::Triton, Link::Bluetooth),
    (0x1304, Model::Triton, Link::Dongle),
    (0x1305, Model::Triton, Link::Dongle),
];

/// The model and link of a USB or Bluetooth HID device, or `None` if it is
/// not a Steam Controller.
pub fn identify(vid: u16, pid: u16) -> Option<(Model, Link)> {
    if vid != VALVE_VID {
        return None;
    }
    PRODUCTS
        .iter()
        .find(|(p, ..)| *p == pid)
        .map(|&(_, m, l)| (m, l))
}

/// Whether a HID interface with this primary usage page and usage is a
/// controller, not one of the emulated keyboard and mouse interfaces (the
/// 2015 controller's lizard mode) or the 2026 puck's pogo-pin interface
/// (vendor page `0xff00`, usage 2, which Linux skips).
pub fn is_controller_interface(usage_page: u32, usage: u32) -> bool {
    const GENERIC_DESKTOP: u32 = 0x01;
    const MOUSE: u32 = 0x02;
    const KEYBOARD: u32 = 0x06;
    let emulated = usage_page == GENERIC_DESKTOP && matches!(usage, MOUSE | KEYBOARD);
    let pogo = usage_page == 0xff00 && usage == 0x02;
    !emulated && !pogo
}

/// The buttons of either controller, under one set of names. The 2015
/// controller's View and Menu are SDL's Back (`STEAM_BUTTON_MENU_MASK`) and
/// Start (`STEAM_BUTTON_ESCAPE_MASK`); the 2026 controller's are as SDL maps
/// them too (Menu to Back, View to Start).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buttons(pub u32);

impl Buttons {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LB: u32 = 1 << 4;
    pub const RB: u32 = 1 << 5;
    /// A trigger pulled all the way (the 2015 controller's click, the 2026
    /// controller's trigger click).
    pub const LT_FULL: u32 = 1 << 6;
    pub const RT_FULL: u32 = 1 << 7;
    pub const BACK: u32 = 1 << 8;
    pub const START: u32 = 1 << 9;
    pub const STEAM: u32 = 1 << 10;
    /// The 2026 controller's quick access button.
    pub const QAM: u32 = 1 << 11;
    pub const L3: u32 = 1 << 12;
    pub const R3: u32 = 1 << 13;
    pub const DPAD_UP: u32 = 1 << 14;
    pub const DPAD_DOWN: u32 = 1 << 15;
    pub const DPAD_LEFT: u32 = 1 << 16;
    pub const DPAD_RIGHT: u32 = 1 << 17;
    /// The 2015 controller's left grip; the 2026 controller's L4 or L5.
    pub const LGRIP: u32 = 1 << 18;
    pub const RGRIP: u32 = 1 << 19;

    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

/// One trackpad. Coordinates are -32768..=32767 with +y up (both drivers
/// negate y for their own down-positive conventions).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Touch {
    pub touched: bool,
    pub clicked: bool,
    pub x: i16,
    pub y: i16,
}

/// What one input report says, in common units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    pub buttons: Buttons,
    /// -32768..=32767, +y up.
    pub left_stick: (i16, i16),
    /// The 2026 controller's right stick (the 2015 one has none).
    pub right_stick: Option<(i16, i16)>,
    /// 0..=1.
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_pad: Touch,
    pub right_pad: Touch,
}

/// What [`Parser::feed`] found in a report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    State(PadState),
    /// A dongle slot or puck slot gained a controller.
    Connected,
    /// It lost it.
    Disconnected,
}

// The 2015 controller, `ValveInReport_t` and `SDL_hidapi_steam.c`.
const D0G_RT: u32 = 0x0000_0001;
const D0G_LT: u32 = 0x0000_0002;
const D0G_RB: u32 = 0x0000_0004;
const D0G_LB: u32 = 0x0000_0008;
const D0G_Y: u32 = 0x0000_0010;
const D0G_B: u32 = 0x0000_0020;
const D0G_X: u32 = 0x0000_0040;
const D0G_A: u32 = 0x0000_0080;
const D0G_DPAD_UP: u32 = 0x0000_0100;
const D0G_DPAD_RIGHT: u32 = 0x0000_0200;
const D0G_DPAD_LEFT: u32 = 0x0000_0400;
const D0G_DPAD_DOWN: u32 = 0x0000_0800;
const D0G_MENU: u32 = 0x0000_1000;
const D0G_STEAM: u32 = 0x0000_2000;
const D0G_ESCAPE: u32 = 0x0000_4000;
const D0G_BACK_LEFT: u32 = 0x0000_8000;
const D0G_BACK_RIGHT: u32 = 0x0001_0000;
const D0G_LPAD_CLICK: u32 = 0x0002_0000;
const D0G_RPAD_CLICK: u32 = 0x0004_0000;
const D0G_LPAD_TOUCH: u32 = 0x0008_0000;
const D0G_RPAD_TOUCH: u32 = 0x0010_0000;
const D0G_JOYSTICK_CLICK: u32 = 0x0040_0000;
const D0G_LPAD_AND_JOYSTICK: u32 = 0x0080_0000;

const IN_REPORT_VERSION: u16 = 0x0001;
const ID_CONTROLLER_STATE: u8 = 1;
const ID_CONTROLLER_WIRELESS: u8 = 3;
const ID_CONTROLLER_BLE_STATE: u8 = 7;
const WIRELESS_DISCONNECTED: u8 = 1;
/// The analog trigger value SDL treats as fully pulled
/// (`STEAMCONTROLLER_TRIGGER_MAX_ANALOG`).
const D0G_TRIGGER_MAX: f32 = 26_000.0;
/// SDL turns the pads 15 degrees (left pad -15, right pad +15) so a thumb's
/// arc reads straight (`RotatePad`).
const D0G_PAD_ROTATION: f32 = 0.261_799;

/// Bluetooth LE: report `0x03`, 20 bytes (report ID, segment header, 18
/// bytes of payload), reassembled into one message.
pub const BLE_REPORT_ID: u8 = 0x03;
pub const BLE_SEGMENT_LEN: usize = 20;
const BLE_PAYLOAD: usize = 18;
const SEGMENT_DATA: u8 = 0x80;
const SEGMENT_LAST: u8 = 0x40;
const BLE_REPORT_STATE: u8 = 4;
const BLE_BUTTONS1: u32 = 0x10;
const BLE_BUTTONS2: u32 = 0x20;
const BLE_BUTTONS3: u32 = 0x40;
const BLE_LEFT_STICK: u32 = 0x80;
const BLE_LEFT_PAD: u32 = 0x100;
const BLE_RIGHT_PAD: u32 = 0x200;

// The 2026 controller, `SDL_hidapi_steam_triton.c` (`TritonButtons`).
const TRI_A: u32 = 0x0000_0001;
const TRI_B: u32 = 0x0000_0002;
const TRI_X: u32 = 0x0000_0004;
const TRI_Y: u32 = 0x0000_0008;
const TRI_QAM: u32 = 0x0000_0010;
const TRI_R3: u32 = 0x0000_0020;
const TRI_VIEW: u32 = 0x0000_0040;
const TRI_R4: u32 = 0x0000_0080;
const TRI_R5: u32 = 0x0000_0100;
const TRI_R: u32 = 0x0000_0200;
const TRI_DPAD_DOWN: u32 = 0x0000_0400;
const TRI_DPAD_RIGHT: u32 = 0x0000_0800;
const TRI_DPAD_LEFT: u32 = 0x0000_1000;
const TRI_DPAD_UP: u32 = 0x0000_2000;
const TRI_MENU: u32 = 0x0000_4000;
const TRI_L3: u32 = 0x0000_8000;
const TRI_STEAM: u32 = 0x0001_0000;
const TRI_L4: u32 = 0x0002_0000;
const TRI_L5: u32 = 0x0004_0000;
const TRI_L: u32 = 0x0008_0000;
const TRI_RPAD_TOUCH: u32 = 0x0020_0000;
const TRI_RPAD_CLICK: u32 = 0x0040_0000;
const TRI_RT_CLICK: u32 = 0x0080_0000;
const TRI_LPAD_TOUCH: u32 = 0x0200_0000;
const TRI_LPAD_CLICK: u32 = 0x0400_0000;
const TRI_LT_CLICK: u32 = 0x0800_0000;

/// Input report IDs (`ETritonReportIDTypes`).
const TRI_STATE: u8 = 0x42;
const TRI_STATE_BLE: u8 = 0x45;
const TRI_WIRELESS_X: u8 = 0x46;
const TRI_STATE_TIMESTAMP: u8 = 0x47;
const TRI_WIRELESS: u8 = 0x79;
/// Report ID plus `TritonMTUNoQuat_t` (45 bytes) or
/// `TritonMTUNoQuat32TS_t` (also 45): the shortest state report SDL takes.
const TRI_STATE_LEN: usize = 46;

fn le16(r: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([r[at], r[at + 1]])
}

fn le32(r: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]])
}

fn map_bits(raw: u32, table: &[(u32, u32)]) -> Buttons {
    Buttons(
        table
            .iter()
            .filter(|(from, _)| raw & from != 0)
            .fold(0, |b, (_, to)| b | to),
    )
}

const D0G_MAP: &[(u32, u32)] = &[
    (D0G_A, Buttons::A),
    (D0G_B, Buttons::B),
    (D0G_X, Buttons::X),
    (D0G_Y, Buttons::Y),
    (D0G_LB, Buttons::LB),
    (D0G_RB, Buttons::RB),
    (D0G_LT, Buttons::LT_FULL),
    (D0G_RT, Buttons::RT_FULL),
    (D0G_MENU, Buttons::BACK),
    (D0G_ESCAPE, Buttons::START),
    (D0G_STEAM, Buttons::STEAM),
    (D0G_JOYSTICK_CLICK, Buttons::L3),
    (D0G_DPAD_UP, Buttons::DPAD_UP),
    (D0G_DPAD_DOWN, Buttons::DPAD_DOWN),
    (D0G_DPAD_LEFT, Buttons::DPAD_LEFT),
    (D0G_DPAD_RIGHT, Buttons::DPAD_RIGHT),
    (D0G_BACK_LEFT, Buttons::LGRIP),
    (D0G_BACK_RIGHT, Buttons::RGRIP),
];

const TRI_MAP: &[(u32, u32)] = &[
    (TRI_A, Buttons::A),
    (TRI_B, Buttons::B),
    (TRI_X, Buttons::X),
    (TRI_Y, Buttons::Y),
    (TRI_L, Buttons::LB),
    (TRI_R, Buttons::RB),
    (TRI_LT_CLICK, Buttons::LT_FULL),
    (TRI_RT_CLICK, Buttons::RT_FULL),
    (TRI_MENU, Buttons::BACK),
    (TRI_VIEW, Buttons::START),
    (TRI_STEAM, Buttons::STEAM),
    (TRI_QAM, Buttons::QAM),
    (TRI_L3, Buttons::L3),
    (TRI_R3, Buttons::R3),
    (TRI_DPAD_UP, Buttons::DPAD_UP),
    (TRI_DPAD_DOWN, Buttons::DPAD_DOWN),
    (TRI_DPAD_LEFT, Buttons::DPAD_LEFT),
    (TRI_DPAD_RIGHT, Buttons::DPAD_RIGHT),
    (TRI_L4 | TRI_L5, Buttons::LGRIP),
    (TRI_R4 | TRI_R5, Buttons::RGRIP),
];

/// Turns pad coordinates by `angle` radians, as SDL's `RotatePad`.
fn rotate(x: i16, y: i16, angle: f32) -> (i16, i16) {
    let (s, c) = angle.sin_cos();
    let (x, y) = (f32::from(x), f32::from(y));
    let clamp = |v: f32| v.round().clamp(-32768.0, 32767.0) as i16;
    (clamp(c * x - s * y), clamp(s * x + c * y))
}

fn d0g_trigger(raw: u8) -> f32 {
    let v = (u32::from(raw) << 7) | u32::from(raw);
    (v as f32 / D0G_TRIGGER_MAX).clamp(0.0, 1.0)
}

/// Reassembles Bluetooth LE segments (SDL's
/// `WriteSegmentToSteamControllerPacketAssembler`).
#[derive(Clone, Debug, Default)]
struct Assembler {
    buf: Vec<u8>,
    next: u8,
}

impl Assembler {
    /// The whole message once its last segment is in.
    fn push(&mut self, seg: &[u8]) -> Option<Vec<u8>> {
        if seg.len() != BLE_SEGMENT_LEN || seg[0] != BLE_REPORT_ID {
            return None;
        }
        let header = seg[1];
        if header & SEGMENT_DATA == 0 {
            return None;
        }
        let n = header & 0x07;
        if n != self.next {
            self.buf.clear();
            self.next = 0;
            if n != 0 {
                return None;
            }
        }
        self.buf.extend_from_slice(&seg[2..]);
        if header & SEGMENT_LAST != 0 {
            self.next = 0;
            return Some(std::mem::take(&mut self.buf));
        }
        self.next += 1;
        None
    }
}

/// Per-device parser. The 2015 controller interleaves its stick and left
/// pad on the same two fields and sends Bluetooth state as deltas, so a
/// parser keeps the last values.
#[derive(Clone, Debug)]
pub struct Parser {
    model: Model,
    link: Link,
    last_packet: Option<u32>,
    prev_stick: (i16, i16),
    prev_left_pad: (i16, i16),
    /// The Bluetooth state so far (chunks only carry what changed).
    ble: BleState,
    assembler: Assembler,
}

#[derive(Clone, Copy, Debug, Default)]
struct BleState {
    raw: u64,
    lt: u8,
    rt: u8,
    stick: (i16, i16),
    lpad: (i16, i16),
    rpad: (i16, i16),
}

impl Parser {
    pub fn new(model: Model, link: Link) -> Self {
        Self {
            model,
            link,
            last_packet: None,
            prev_stick: (0, 0),
            prev_left_pad: (0, 0),
            ble: BleState::default(),
            assembler: Assembler::default(),
        }
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn link(&self) -> Link {
        self.link
    }

    /// Parses one input report. `None` for reports that say nothing we use
    /// (battery, debug, a repeated packet number, a partial segment).
    pub fn feed(&mut self, report: &[u8]) -> Option<Event> {
        match self.model {
            Model::Triton => parse_triton(report),
            Model::Original
                if self.link == Link::Bluetooth && report.first() == Some(&BLE_REPORT_ID) =>
            {
                let msg = self.assembler.push(report)?;
                self.original(&msg)
            }
            Model::Original => self.original(report),
        }
    }

    fn original(&mut self, r: &[u8]) -> Option<Event> {
        if r.len() >= 4 && u16::from_le_bytes([r[0], r[1]]) == IN_REPORT_VERSION {
            return match r[2] {
                ID_CONTROLLER_STATE | ID_CONTROLLER_BLE_STATE => self.original_state(r),
                // SDL's `D0G_IS_VALID_WIRELESS_EVENT`: a length of at least
                // one, then the event type.
                ID_CONTROLLER_WIRELESS if r.len() >= 5 && r[3] >= 1 => {
                    Some(if r[4] == WIRELESS_DISCONNECTED {
                        Event::Disconnected
                    } else {
                        Event::Connected
                    })
                }
                _ => None,
            };
        }
        if r.first().is_some_and(|b| b & 0x0f == BLE_REPORT_STATE) {
            return self.ble_state(r);
        }
        None
    }

    /// `ValveControllerStatePacket_t` after the 4-byte header:
    /// packet number at 4, button and trigger bytes at 8..16 (the left and
    /// right triggers at 11 and 12), left pad or stick at 16, right pad at 20.
    fn original_state(&mut self, r: &[u8]) -> Option<Event> {
        if r.len() < 24 {
            return None;
        }
        let packet = le32(r, 4);
        if self.last_packet == Some(packet) {
            return None;
        }
        self.last_packet = Some(packet);
        let raw = u32::from_le_bytes([r[8], r[9], r[10], 0]);
        let left = (le16(r, 16), le16(r, 18));
        let right = (le16(r, 20), le16(r, 22));
        Some(Event::State(self.d0g_state(raw, r[11], r[12], left, right)))
    }

    /// SDL's `FormatStatePacketUntilGyro`: the finger-down bit says whether
    /// the shared left fields hold the pad or the stick; with both in use the
    /// controller alternates and the other keeps its previous value.
    fn d0g_state(
        &mut self,
        raw: u32,
        lt: u8,
        rt: u8,
        left: (i16, i16),
        right: (i16, i16),
    ) -> PadState {
        let mut raw = raw;
        let both = raw & D0G_LPAD_AND_JOYSTICK != 0;
        let (stick, lpad);
        if raw & D0G_LPAD_TOUCH != 0 {
            lpad = left;
            self.prev_left_pad = left;
            stick = if both { self.prev_stick } else { (0, 0) };
            if !both {
                self.prev_stick = (0, 0);
            }
        } else {
            stick = left;
            self.prev_stick = left;
            if both {
                lpad = self.prev_left_pad;
            } else {
                lpad = (0, 0);
                self.prev_left_pad = (0, 0);
                // Older firmware reports the stick click as a pad click
                // while the pad is idle.
                if raw & D0G_LPAD_CLICK != 0 {
                    raw = (raw & !D0G_LPAD_CLICK) | D0G_JOYSTICK_CLICK;
                }
            }
        }
        if both {
            raw |= D0G_LPAD_TOUCH;
        }
        let (lx, ly) = rotate(lpad.0, lpad.1, -D0G_PAD_ROTATION);
        let (rx, ry) = rotate(right.0, right.1, D0G_PAD_ROTATION);
        PadState {
            buttons: map_bits(raw, D0G_MAP),
            left_stick: stick,
            right_stick: None,
            left_trigger: d0g_trigger(lt),
            right_trigger: d0g_trigger(rt),
            left_pad: Touch {
                touched: raw & D0G_LPAD_TOUCH != 0,
                clicked: raw & D0G_LPAD_CLICK != 0,
                x: lx,
                y: ly,
            },
            right_pad: Touch {
                touched: raw & D0G_RPAD_TOUCH != 0,
                clicked: raw & D0G_RPAD_CLICK != 0,
                x: rx,
                y: ry,
            },
        }
    }

    /// SDL's `UpdateBLESteamControllerState`: a chunk mask (the high nibble
    /// of byte 0 and all of byte 1), then the chunks present, in order.
    fn ble_state(&mut self, r: &[u8]) -> Option<Event> {
        if r.len() < 2 {
            return None;
        }
        let mask = u32::from(r[0] & 0xf0) | u32::from(r[1]) << 8;
        let mut at = 2;
        let mut take = |n: usize| -> Option<&[u8]> {
            let s = r.get(at..at + n)?;
            at += n;
            Some(s)
        };
        let s = &mut self.ble;
        if mask & BLE_BUTTONS1 != 0 {
            let b = take(3)?;
            s.raw = (s.raw & !0xff_ffff)
                | u64::from(b[0])
                | u64::from(b[1]) << 8
                | u64::from(b[2]) << 16;
        }
        if mask & BLE_BUTTONS2 != 0 {
            let b = take(2)?;
            (s.lt, s.rt) = (b[0], b[1]);
        }
        if mask & BLE_BUTTONS3 != 0 {
            let b = take(3)?;
            s.raw = (s.raw & !(0xff_ffff << 40))
                | u64::from(b[0]) << 40
                | u64::from(b[1]) << 48
                | u64::from(b[2]) << 56;
        }
        let pair = |b: &[u8]| (le16(b, 0), le16(b, 2));
        if mask & BLE_LEFT_STICK != 0 {
            s.stick = pair(take(4)?);
        }
        if mask & BLE_LEFT_PAD != 0 {
            s.lpad = pair(take(4)?);
        }
        if mask & BLE_RIGHT_PAD != 0 {
            s.rpad = pair(take(4)?);
        }
        let s = *s;
        let raw = s.raw as u32 & 0xff_ffff;
        let (lx, ly) = rotate(s.lpad.0, s.lpad.1, -D0G_PAD_ROTATION);
        let (rx, ry) = rotate(s.rpad.0, s.rpad.1, D0G_PAD_ROTATION);
        Some(Event::State(PadState {
            buttons: map_bits(raw, D0G_MAP),
            left_stick: s.stick,
            right_stick: None,
            left_trigger: d0g_trigger(s.lt),
            right_trigger: d0g_trigger(s.rt),
            left_pad: Touch {
                touched: raw & D0G_LPAD_TOUCH != 0,
                clicked: raw & D0G_LPAD_CLICK != 0,
                x: lx,
                y: ly,
            },
            right_pad: Touch {
                touched: raw & D0G_RPAD_TOUCH != 0,
                clicked: raw & D0G_RPAD_CLICK != 0,
                x: rx,
                y: ry,
            },
        }))
    }
}

/// The 2026 controller's reports (`TritonMTUNoQuat_t` and
/// `TritonMTUNoQuat32TS_t` after the report ID): sequence at 1, buttons at
/// 2..6, triggers at 6 and 8, sticks at 10..18; then the left pad (x, y,
/// pressure) and the right pad, at 18 and 24, or at 20 and 26 in report
/// `0x47`, which puts a trackpad timestamp first.
fn parse_triton(r: &[u8]) -> Option<Event> {
    match *r.first()? {
        TRI_WIRELESS | TRI_WIRELESS_X if r.len() >= 2 => match r[1] {
            1 => Some(Event::Disconnected),
            2 => Some(Event::Connected),
            _ => None,
        },
        id @ (TRI_STATE | TRI_STATE_BLE | TRI_STATE_TIMESTAMP) if r.len() >= TRI_STATE_LEN => {
            let raw = le32(r, 2);
            let pads = if id == TRI_STATE_TIMESTAMP { 20 } else { 18 };
            let trigger = |at| (f32::from(le16(r, at)) / 32767.0).clamp(0.0, 1.0);
            Some(Event::State(PadState {
                buttons: map_bits(raw, TRI_MAP),
                left_stick: (le16(r, 10), le16(r, 12)),
                right_stick: Some((le16(r, 14), le16(r, 16))),
                left_trigger: trigger(6),
                right_trigger: trigger(8),
                left_pad: Touch {
                    touched: raw & TRI_LPAD_TOUCH != 0,
                    clicked: raw & TRI_LPAD_CLICK != 0,
                    x: le16(r, pads),
                    y: le16(r, pads + 2),
                },
                right_pad: Touch {
                    touched: raw & TRI_RPAD_TOUCH != 0,
                    clicked: raw & TRI_RPAD_CLICK != 0,
                    x: le16(r, pads + 6),
                    y: le16(r, pads + 8),
                },
            }))
        }
        _ => None,
    }
}

/// A report for the host to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidWrite {
    /// A feature report (`SetReport` type feature) rather than an output
    /// report.
    pub feature: bool,
    /// The report ID first. ID 0 means the device does not number its
    /// reports; the host sends the bytes after it (as hidapi does).
    pub data: Vec<u8>,
}

/// Feature report command IDs (`FeatureReportMessageIDs`).
const ID_CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
const ID_SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
const ID_SET_SETTINGS_VALUES: u8 = 0x87;
const ID_LOAD_DEFAULT_SETTINGS: u8 = 0x8e;
const ID_TRIGGER_HAPTIC_PULSE: u8 = 0x8f;

/// Setting numbers (`ControllerSettings`; Linux numbers them the same).
const SETTING_LEFT_TRACKPAD_MODE: u8 = 7;
const SETTING_RIGHT_TRACKPAD_MODE: u8 = 8;
const SETTING_LIZARD_MODE: u8 = 9;
const SETTING_SMOOTH_ABSOLUTE_MOUSE: u8 = 24;
const SETTING_WIRELESS_PACKET_VERSION: u8 = 49;
const TRACKPAD_ABSOLUTE_MOUSE: u16 = 0;
const TRACKPAD_NONE: u16 = 7;
const LIZARD_MODE_OFF: u16 = 0;

/// The 2015 controller's feature reports are 64 bytes after report ID 0
/// (SDL sends 65 with the ID); the 2026 controller's are report 1, 64
/// bytes with the ID.
const D0G_FEATURE_LEN: usize = 65;
const TRI_FEATURE_ID: u8 = 1;
const TRI_FEATURE_LEN: usize = 64;

/// A feature message: command, payload length, payload.
fn feature(model: Model, cmd: u8, payload: &[u8]) -> HidWrite {
    let (id, len) = match model {
        Model::Original => (0, D0G_FEATURE_LEN),
        Model::Triton => (TRI_FEATURE_ID, TRI_FEATURE_LEN),
    };
    let mut data = vec![0u8; len];
    data[0] = id;
    data[1] = cmd;
    data[2] = payload.len() as u8;
    data[3..3 + payload.len()].copy_from_slice(payload);
    HidWrite {
        feature: true,
        data,
    }
}

fn settings(model: Model, values: &[(u8, u16)]) -> HidWrite {
    let payload: Vec<u8> = values
        .iter()
        .flat_map(|&(n, v)| {
            let [lo, hi] = v.to_le_bytes();
            [n, lo, hi]
        })
        .collect();
    feature(model, ID_SET_SETTINGS_VALUES, &payload)
}

/// The reports that take the controller out of lizard mode (its keyboard
/// and mouse emulation) so it is only a game controller while we hold it.
///
/// 2015: SDL's `ResetSteamController` without the attribute query: clear
/// the digital mappings, load the default settings, then set the wireless
/// packet version 2 and both trackpads to `TRACKPAD_NONE` with smoothing
/// off. 2026: SDL's `MaintainTritonState`, `SETTING_LIZARD_MODE` off; the
/// firmware's watchdog turns lizard mode back on unless it is sent again
/// within a few seconds ([`keepalive`]).
pub fn lizard_off(model: Model) -> Vec<HidWrite> {
    match model {
        Model::Original => vec![
            feature(model, ID_CLEAR_DIGITAL_MAPPINGS, &[]),
            feature(model, ID_LOAD_DEFAULT_SETTINGS, &[]),
            settings(
                model,
                &[
                    (SETTING_WIRELESS_PACKET_VERSION, 2),
                    (SETTING_LEFT_TRACKPAD_MODE, TRACKPAD_NONE),
                    (SETTING_RIGHT_TRACKPAD_MODE, TRACKPAD_NONE),
                    (SETTING_SMOOTH_ABSOLUTE_MOUSE, 0),
                ],
            ),
        ],
        Model::Triton => vec![settings(model, &[(SETTING_LIZARD_MODE, LIZARD_MODE_OFF)])],
    }
}

/// How often the 2026 controller needs lizard mode turned off again (SDL
/// sends it every 3 s).
pub const KEEPALIVE: Duration = Duration::from_secs(3);

/// What to send every [`KEEPALIVE`] while we hold the controller.
pub fn keepalive(model: Model) -> Option<HidWrite> {
    match model {
        Model::Original => None,
        Model::Triton => lizard_off(model).pop(),
    }
}

/// The reports that give lizard mode back on release.
///
/// 2015: SDL's `CloseSteamController`: default digital mappings, default
/// settings, and the right trackpad back to `TRACKPAD_ABSOLUTE_MOUSE`.
/// 2026: Linux's `steam_set_lizard_mode(true)`: default digital mappings
/// and default settings (the watchdog would also bring it back).
pub fn lizard_on(model: Model) -> Vec<HidWrite> {
    let mut out = vec![
        feature(model, ID_SET_DEFAULT_DIGITAL_MAPPINGS, &[]),
        feature(model, ID_LOAD_DEFAULT_SETTINGS, &[]),
    ];
    if model == Model::Original {
        out.push(settings(
            model,
            &[(SETTING_RIGHT_TRACKPAD_MODE, TRACKPAD_ABSOLUTE_MOUSE)],
        ));
    }
    out
}

/// A trackpad actuator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Left,
    Right,
    /// Both actuators in one report (Linux's `STEAM_PAD_BOTH`).
    Both,
}

impl Side {
    /// The pad number on the wire. Linux's `steam_haptic_pulse`: left and
    /// right are swapped on this report for legacy reasons, so right is 0
    /// and left 1, on both controllers; both is 2.
    fn wire(self) -> u8 {
        match self {
            Side::Right => 0,
            Side::Left => 1,
            Side::Both => 2,
        }
    }
}

/// One train of haptic pulses: `count` pulses of `on_us` with `off_us`
/// between them. A train at a steady period is a tone at
/// `1e6 / (on_us + off_us)` Hz, which is how the trackpads play notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PulseTrain {
    pub on_us: u16,
    pub off_us: u16,
    pub count: u16,
    /// dB, -24..=6 (Linux); the 2026 report has no gain field.
    pub gain_db: i8,
}

/// The haptic pulse report for one pad.
///
/// 2015: feature `ID_TRIGGER_HAPTIC_PULSE` (`0x8f`), 8 payload bytes: pad,
/// on and off time and count (little-endian u16) and gain, as Linux sends
/// it. 2026: output report `0x81` (`MsgHapticPulse`): side, on, off, count,
/// 8 bytes with the ID.
pub fn haptic_pulse(model: Model, side: Side, t: PulseTrain) -> HidWrite {
    let [on0, on1] = t.on_us.to_le_bytes();
    let [off0, off1] = t.off_us.to_le_bytes();
    let [n0, n1] = t.count.to_le_bytes();
    match model {
        Model::Original => feature(
            model,
            ID_TRIGGER_HAPTIC_PULSE,
            &[side.wire(), on0, on1, off0, off1, n0, n1, t.gain_db as u8],
        ),
        Model::Triton => HidWrite {
            feature: false,
            data: vec![0x81, side.wire(), on0, on1, off0, off1, n0, n1],
        },
    }
}

/// The 2026 controller's rumble (output report `0x80`, `MsgHapticRumble`,
/// 10 bytes): type 0, intensity 0, then each side's speed and gain, as SDL
/// sends it. The heavy (left) Xbox motor goes to the left actuator. The
/// firmware stops after about 50 ms without a new one; SDL resends every
/// [`TRITON_RUMBLE_RESEND`].
pub fn triton_rumble(left: u16, right: u16) -> HidWrite {
    let [l0, l1] = left.to_le_bytes();
    let [r0, r1] = right.to_le_bytes();
    HidWrite {
        feature: false,
        data: vec![0x80, 0, 0, 0, l0, l1, 0, r0, r1, 0],
    }
}

pub const TRITON_RUMBLE_RESEND: Duration = Duration::from_millis(40);

/// A feature report for the 2015 controller over Bluetooth LE, cut into
/// report `0x03` segments of 18 bytes (SDL's `SetFeatureReport`): segment
/// header `0x80 | n`, plus `0x40` on the last. The message is the command,
/// its length byte and the payload.
pub fn ble_segments(w: &HidWrite) -> Vec<HidWrite> {
    let msg_len = (3 + usize::from(w.data[2])).min(w.data.len()) - 1;
    let msg = &w.data[1..1 + msg_len];
    let chunks: Vec<&[u8]> = msg.chunks(BLE_PAYLOAD).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut data = vec![0u8; BLE_SEGMENT_LEN];
            data[0] = BLE_REPORT_ID;
            data[1] = SEGMENT_DATA
                | (i as u8 & 0x07)
                | if i + 1 == chunks.len() {
                    SEGMENT_LAST
                } else {
                    0
                };
            data[2..2 + c.len()].copy_from_slice(c);
            HidWrite {
                feature: true,
                data,
            }
        })
        .collect()
}

/// What actually goes on the wire for `w` on this link: Bluetooth feature
/// reports of the 2015 controller are segmented, everything else as is.
pub fn on_link(model: Model, link: Link, w: HidWrite) -> Vec<HidWrite> {
    if model == Model::Original && link == Link::Bluetooth && w.feature {
        ble_segments(&w)
    } else {
        vec![w]
    }
}

#[cfg(test)]
#[path = "steam_tests.rs"]
mod tests;
