//! Physical gamepads through Apple's GameController framework (`GCController`).
//!
//! Split in two so the logic is testable without hardware or even macOS:
//!
//! * this file: [`ControllerSource`] (what a platform supplies: a list of
//!   [`RawPad`] snapshots and a rumble sink), [`map_snapshot`] (framework
//!   vocabulary to [`HostPadState`]) and [`GameControllerPads`], the
//!   [`HostBackend`] over any source;
//! * `apple.rs` (macOS only): [`AppleSource`], the `objc2-game-controller`
//!   implementation, including CoreHaptics rumble and PlayStation lightbars.
//!
//! Port handling, hot-plug and the insertion/removal masks live in
//! [`InputManager`](crate::InputManager): it assigns the first four pads in
//! the order this backend reports them (stable: connection order) to ports 0-3
//! and raises `device_changes()` bits as pads appear and vanish. A pad that
//! is unplugged and replugged gets a fresh [`HostPadId`], so the manager
//! reports the removal and the insertion even when both fall between two
//! syncs.
//!
//! Do not add this backend alongside [`GilrsPads`](super::gilrs_pads::GilrsPads)
//! on macOS: both see the same physical pad and it would take two ports.

use crate::backend::HostBackend;
use crate::gamepad::Rumble;
use crate::host::{HostButton, HostPad, HostPadId, HostPadState, Touchpad};
use crate::report::DeviceInfo;

#[cfg(target_vendor = "apple")]
mod apple;
pub mod remote;
#[cfg(target_vendor = "apple")]
pub use apple::{AppleSource, service_main_loop};

/// A controller's inputs in GameController terms, already read out as plain
/// numbers. Sticks are -1..=1 with +Y up (the framework's convention),
/// triggers and buttons 0..=1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub a: f32,
    pub b: f32,
    pub x: f32,
    pub y: f32,
    pub left_shoulder: f32,
    pub right_shoulder: f32,
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    /// `buttonMenu`.
    pub menu: bool,
    /// `buttonOptions` (absent on some pads, then false).
    pub options: bool,
    pub left_stick_press: bool,
    pub right_stick_press: bool,
    /// A DualSense's or DualShock 4's touchpad (`GCDualSenseGamepad`,
    /// `GCDualShockGamepad`): the click and the two finger positions as
    /// read, a lifted finger at exactly (0, 0) ([`touch_finger`]).
    pub touchpad: Option<RawTouchpad>,
}

/// A touchpad as the framework reports it: `touchpadButton`, and
/// `touchpadPrimary` / `touchpadSecondary` as x, y in -1..=1 (+y up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawTouchpad {
    pub click: bool,
    pub primary: (f32, f32),
    pub secondary: (f32, f32),
}

/// A finger from a touchpad axis pair. The framework has no "touching"
/// flag on these elements and returns a lifted finger to the centre, so
/// exactly (0, 0) reads as no finger (a resting finger is never exactly
/// there for long); anything not finite too.
pub fn touch_finger(p: (f32, f32)) -> Option<(f32, f32)> {
    let ok = p.0.is_finite() && p.1.is_finite() && p != (0.0, 0.0);
    ok.then(|| (p.0.clamp(-1.0, 1.0), p.1.clamp(-1.0, 1.0)))
}

impl RawTouchpad {
    pub fn to_touchpad(self) -> Touchpad {
        Touchpad {
            click: self.click,
            primary: touch_finger(self.primary),
            secondary: touch_finger(self.secondary),
        }
    }
}

/// Which GameController profile a controller came with: the class its
/// `extendedGamepad` is (a DualSense's and a DualShock 4's carry the
/// touchpad), or a remote's micro gamepad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// `GCExtendedGamepad` and no subclass we know: no touchpad.
    #[default]
    Extended,
    /// `GCDualSenseGamepad`.
    DualSense,
    /// `GCDualShockGamepad`.
    DualShock4,
    /// `GCMicroGamepad` only (an Apple TV remote).
    Micro,
}

impl Profile {
    /// The word `h2 agent hostpads` shows.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Extended => "extended",
            Profile::DualSense => "dualsense",
            Profile::DualShock4 => "dualshock4",
            Profile::Micro => "micro",
        }
    }
}

/// One connected controller as a platform source reports it.
#[derive(Clone, Debug)]
pub struct RawPad {
    /// Unique per connection: never reused, so a replug yields a new key.
    pub key: u64,
    pub name: String,
    pub profile: Profile,
    /// The framework's `productCategory` ("DualShock 4", ...), if any.
    pub category: Option<String>,
    pub snapshot: Snapshot,
    pub supports_rumble: bool,
    /// An Apple TV remote (a micro gamepad): `snapshot` is unused and this
    /// is read instead ([`remote`]).
    pub remote: Option<remote::RemoteSnapshot>,
}

/// A platform's controller list.
pub trait ControllerSource: Send {
    /// Currently connected controllers in connection order.
    fn poll(&mut self) -> Vec<RawPad>;
    /// Motor speeds 0..=1 (`left` is the heavy motor). 0/0 stops.
    fn set_rumble(&mut self, key: u64, left: f32, right: f32);
    /// Set the PlayStation lightbar for this connection. `None` restores its original colour.
    fn set_lightbar(&mut self, _key: u64, _rgb: Option<[u8; 3]>) {}
}

/// A haptic locality a controller may advertise (`GCDeviceHaptics.supportedLocalities`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    LeftHandle,
    RightHandle,
    Handles,
    All,
    Default,
}

/// How rumble is delivered on one controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RumblePlan {
    /// One engine per handle: left is the heavy motor, right the light one.
    Split,
    /// A single engine on this locality drives both; it plays at the larger of
    /// the two requested speeds.
    Combined(Locality),
}

/// Chooses how to rumble given what the controller supports: separate
/// handles when both exist, else one engine on the first of handles / all /
/// default. `None` means no usable haptics, so the pad reports no rumble.
pub fn plan_rumble(supported: &[Locality]) -> Option<RumblePlan> {
    let has = |l| supported.contains(&l);
    if has(Locality::LeftHandle) && has(Locality::RightHandle) {
        return Some(RumblePlan::Split);
    }
    [Locality::Handles, Locality::All, Locality::Default]
        .into_iter()
        .find(|&l| has(l))
        .map(RumblePlan::Combined)
}

/// Pressure at or above which a pressure-sensitive button counts as a
/// digital press (the d-pad has no pressure; stick clicks use the framework's
/// own pressed state).
const PRESS_THRESHOLD: f32 = 0.5;

fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Maps a GameController snapshot onto the Xbox-flavoured [`HostPadState`].
///
/// * A/B/X/Y keep their physical position (south/east/west/north), matching
///   the Xbox layout on Xbox-style pads.
/// * Right shoulder is Black and left shoulder is White, as everywhere else
///   in this crate; both are reported as pressure.
/// * Triggers and the six analog buttons carry the framework's 0..1
///   pressure, which `to_gamepad` rescales to 0..=255.
/// * Menu is Start, Options is Back.
pub fn map_snapshot(s: &Snapshot) -> HostPadState {
    let mut st = HostPadState::default();
    let mut digital = |hb: HostButton, down: bool| st.buttons.set(hb, down);
    digital(HostButton::South, unit(s.a) >= PRESS_THRESHOLD);
    digital(HostButton::East, unit(s.b) >= PRESS_THRESHOLD);
    digital(HostButton::West, unit(s.x) >= PRESS_THRESHOLD);
    digital(HostButton::North, unit(s.y) >= PRESS_THRESHOLD);
    digital(
        HostButton::RightBumper,
        unit(s.right_shoulder) >= PRESS_THRESHOLD,
    );
    digital(
        HostButton::LeftBumper,
        unit(s.left_shoulder) >= PRESS_THRESHOLD,
    );
    digital(HostButton::Start, s.menu);
    digital(HostButton::Back, s.options);
    digital(HostButton::LeftStick, s.left_stick_press);
    digital(HostButton::RightStick, s.right_stick_press);
    digital(HostButton::DpadUp, s.dpad_up);
    digital(HostButton::DpadDown, s.dpad_down);
    digital(HostButton::DpadLeft, s.dpad_left);
    digital(HostButton::DpadRight, s.dpad_right);
    st.left_stick = s.left_stick;
    st.right_stick = s.right_stick;
    st.left_trigger = s.left_trigger;
    st.right_trigger = s.right_trigger;
    // The framework already applies its own deadzones.
    st.prefiltered = true;
    st.touchpad = s.touchpad.map(RawTouchpad::to_touchpad);
    st.analog_buttons = Some([s.a, s.b, s.x, s.y, s.right_shoulder, s.left_shoulder].map(unit));
    st
}

/// [`HostBackend`] over a [`ControllerSource`].
///
/// An Apple TV remote is merged into the first gamepad, so both drive the
/// same player at once; with no gamepad it is a pad of its own. The merged
/// pad takes the remote's name while the remote is pressing something, so
/// the last-used device (`crate::active`) can tell them apart.
pub struct GameControllerPads<S: ControllerSource> {
    source: S,
    navs: std::collections::HashMap<u64, remote::RemoteNav>,
    epoch: std::time::Instant,
    /// Each controller as of the last poll, before the remote merge
    /// ([`HostBackend::devices`]).
    devices: Vec<DeviceInfo>,
}

impl<S: ControllerSource> GameControllerPads<S> {
    pub fn with_source(source: S) -> Self {
        Self {
            source,
            navs: Default::default(),
            epoch: std::time::Instant::now(),
            devices: Vec::new(),
        }
    }

    /// [`HostBackend::poll`] at `now` on the remote clock.
    fn poll_at(&mut self, now: std::time::Duration) -> Vec<HostPad> {
        let raw = self.source.poll();
        self.navs.retain(|k, _| raw.iter().any(|p| p.key == *k && p.remote.is_some()));
        let mut pads = Vec::new();
        let mut remotes = Vec::new();
        self.devices.clear();
        for p in raw {
            let pad = |state| HostPad {
                id: HostPadId(p.key),
                name: p.name.clone(),
                state,
                physical: true,
                supports_rumble: p.supports_rumble,
            };
            let state = match &p.remote {
                Some(r) => self.navs.entry(p.key).or_default().update(r, now),
                None => map_snapshot(&p.snapshot),
            };
            self.devices.push(DeviceInfo {
                backend: "game-controller".into(),
                id: p.key,
                name: p.name.clone(),
                profile: Some(p.profile.name().into()),
                category: p.category.clone(),
                state,
            });
            match &p.remote {
                Some(_) => remotes.push(pad(state)),
                None => pads.push(pad(state)),
            }
        }
        pads.sort_by_key(|p| p.id); // keys grow with connection order
        remotes.sort_by_key(|p| p.id);
        match pads.first_mut() {
            Some(first) => {
                for r in &remotes {
                    remote::merge_remote(&mut first.state, &r.state);
                    if remote::remote_active(&r.state) {
                        first.name = r.name.clone();
                    }
                }
            }
            None => pads = remotes,
        }
        pads
    }
}

#[cfg(target_vendor = "apple")]
impl GameControllerPads<AppleSource> {
    /// The system GameController backend. Also asks the framework to keep
    /// delivering input while the app is in the background, which a headless
    /// or windowless recomp host needs.
    pub fn new() -> Self {
        Self::with_source(AppleSource::new())
    }
}

#[cfg(target_vendor = "apple")]
impl Default for GameControllerPads<AppleSource> {
    fn default() -> Self {
        Self::new()
    }
}

fn to_u16_speed(v: u16) -> f32 {
    f32::from(v) / 65535.0
}

impl<S: ControllerSource> HostBackend for GameControllerPads<S> {
    fn name(&self) -> &str {
        "game-controller"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        let now = self.epoch.elapsed();
        self.poll_at(now)
    }

    fn set_rumble(&mut self, id: HostPadId, rumble: Rumble) {
        self.source.set_rumble(
            id.0,
            to_u16_speed(rumble.left_motor),
            to_u16_speed(rumble.right_motor),
        );
    }

    fn set_lightbar(&mut self, id: HostPadId, rgb: Option<[u8; 3]>) {
        self.source.set_lightbar(id.0, rgb);
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        self.devices.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::FeedbackRequest;
    use crate::gamepad::{AnalogButton, Buttons};
    use crate::host::ConvertConfig;
    use crate::{InputManager, PortIndex};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Fake {
        pads: Vec<RawPad>,
        rumble: Vec<(u64, f32, f32)>,
        lightbar: Vec<(u64, Option<[u8; 3]>)>,
    }
    #[derive(Clone, Default)]
    struct FakeSource(Arc<Mutex<Fake>>);
    impl ControllerSource for FakeSource {
        fn poll(&mut self) -> Vec<RawPad> {
            self.0.lock().unwrap().pads.clone()
        }
        fn set_rumble(&mut self, key: u64, l: f32, r: f32) {
            self.0.lock().unwrap().rumble.push((key, l, r));
        }
        fn set_lightbar(&mut self, key: u64, rgb: Option<[u8; 3]>) {
            self.0.lock().unwrap().lightbar.push((key, rgb));
        }
    }
    fn raw(key: u64, snapshot: Snapshot) -> RawPad {
        RawPad {
            key,
            name: format!("pad{key}"),
            profile: Profile::Extended,
            category: None,
            snapshot,
            supports_rumble: true,
            remote: None,
        }
    }

    fn remote(key: u64, r: remote::RemoteSnapshot) -> RawPad {
        RawPad {
            key,
            name: remote::NAME.into(),
            profile: Profile::Micro,
            category: None,
            snapshot: Snapshot::default(),
            supports_rumble: false,
            remote: Some(r),
        }
    }

    #[test]
    fn the_remote_merges_into_the_first_gamepad_and_names_it_while_used() {
        use std::time::Duration;
        let src = FakeSource::default();
        src.0.lock().unwrap().pads = vec![
            raw(2, Snapshot { x: 1.0, ..Default::default() }),
            remote(5, remote::RemoteSnapshot { select: true, ..Default::default() }),
            raw(3, Snapshot::default()),
        ];
        let mut b = GameControllerPads::with_source(src.clone());
        let pads = b.poll_at(Duration::from_millis(1));
        assert_eq!(pads.len(), 2, "the remote is not a pad of its own");
        assert_eq!(pads[0].id, HostPadId(2));
        assert_eq!(pads[0].name, "Apple TV Remote", "the remote is in use");
        assert!(pads[0].state.buttons.contains(HostButton::South), "the remote's click");
        assert!(pads[0].state.buttons.contains(HostButton::West), "the pad's own X");
        assert!(!pads[1].state.buttons.contains(HostButton::South), "only the first pad");
        src.0.lock().unwrap().pads[1].remote = Some(remote::RemoteSnapshot::default());
        let pads = b.poll_at(Duration::from_millis(2));
        assert_eq!(pads[0].name, "pad2", "an idle remote leaves the name");
    }

    #[test]
    fn devices_describe_each_controller_before_the_remote_merge() {
        use std::time::Duration;
        let src = FakeSource::default();
        let ds4 = RawPad {
            profile: Profile::DualShock4,
            category: Some("DualShock 4".into()),
            ..raw(
                4,
                Snapshot {
                    touchpad: Some(RawTouchpad {
                        click: true,
                        primary: (0.25, 0.5),
                        secondary: (0.0, 0.0),
                    }),
                    ..Default::default()
                },
            )
        };
        src.0.lock().unwrap().pads = vec![
            ds4,
            remote(5, remote::RemoteSnapshot { select: true, ..Default::default() }),
        ];
        let mut b = GameControllerPads::with_source(src.clone());
        assert!(b.devices().is_empty(), "nothing before the first poll");
        let pads = b.poll_at(Duration::from_millis(1));
        assert_eq!(pads.len(), 1, "the remote merged");
        let d = b.devices();
        assert_eq!(d.len(), 2, "but both are described");
        assert_eq!((d[0].id, d[0].profile.as_deref()), (4, Some("dualshock4")));
        assert_eq!(d[0].category.as_deref(), Some("DualShock 4"));
        assert_eq!(d[0].backend, "game-controller");
        assert_eq!(
            d[0].state.touchpad,
            Some(Touchpad {
                click: true,
                primary: Some((0.25, 0.5)),
                secondary: None,
            })
        );
        assert_eq!(d[1].profile.as_deref(), Some("micro"));
        assert!(d[1].state.buttons.contains(HostButton::South), "the remote's own click");
        src.0.lock().unwrap().pads.clear();
        b.poll_at(Duration::from_millis(2));
        assert!(b.devices().is_empty(), "unplugged");
    }

    #[test]
    fn a_remote_alone_is_its_own_pad() {
        use std::time::Duration;
        let src = FakeSource::default();
        src.0.lock().unwrap().pads = vec![remote(7, remote::RemoteSnapshot { back: true, ..Default::default() })];
        let mut b = GameControllerPads::with_source(src.clone());
        let pads = b.poll_at(Duration::from_millis(1));
        assert_eq!(pads.len(), 1);
        assert_eq!(pads[0].id, HostPadId(7));
        assert!(!pads[0].supports_rumble);
        let g = pads[0].state.to_gamepad(&ConvertConfig::default(), true);
        assert_eq!(g.analog(AnalogButton::B), 255, "Back is B");
        // Through the manager: a pad on port 0 that the menus can read.
        let mut m = InputManager::new(ConvertConfig::default()).unwrap();
        m.add_backend(Box::new(GameControllerPads::with_source(src)));
        m.sync();
        let h = m.open(PortIndex::new(0).unwrap()).unwrap();
        assert_eq!(m.get_state(h).unwrap().gamepad.analog(AnalogButton::B), 255);
    }
    fn gp(s: &Snapshot) -> crate::gamepad::Gamepad {
        map_snapshot(s).to_gamepad(&ConvertConfig::default(), true)
    }

    #[test]
    fn face_buttons_and_pressure() {
        let g = gp(&Snapshot {
            a: 1.0,
            b: 0.25,
            x: 0.5,
            ..Default::default()
        });
        assert_eq!(g.analog(AnalogButton::A), 255);
        assert_eq!(g.analog(AnalogButton::B), 64);
        assert_eq!(g.analog(AnalogButton::X), 128);
        assert_eq!(g.analog(AnalogButton::Y), 0);
    }

    #[test]
    fn shoulders_are_black_and_white() {
        let g = gp(&Snapshot {
            right_shoulder: 1.0,
            left_shoulder: 0.5,
            ..Default::default()
        });
        assert_eq!(g.analog(AnalogButton::Black), 255);
        assert_eq!(g.analog(AnalogButton::White), 128);
    }

    #[test]
    fn triggers_span_full_range() {
        let g = gp(&Snapshot {
            left_trigger: 1.0,
            right_trigger: 0.5,
            ..Default::default()
        });
        assert_eq!(g.analog(AnalogButton::LeftTrigger), 255);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 128); // no extra trigger deadzone
        let g = gp(&Snapshot::default());
        assert_eq!(g.analog, [0; 8]);
    }

    #[test]
    fn sticks_span_xbox_range_y_up() {
        let g = gp(&Snapshot {
            left_stick: (1.0, 1.0),
            right_stick: (-1.0, -1.0),
            ..Default::default()
        });
        // Framework-calibrated axes are passed through independently.
        assert_eq!((g.thumb_lx, g.thumb_ly), (32767, 32767));
        assert_eq!((g.thumb_rx, g.thumb_ry), (-32768, -32768));
        let g = gp(&Snapshot {
            left_stick: (0.0, 1.0),
            right_stick: (-1.0, 0.0),
            ..Default::default()
        });
        assert_eq!((g.thumb_ly, g.thumb_rx), (32767, -32768));
        // Already filtered by the framework: small values pass through, scaled.
        let g = gp(&Snapshot {
            left_stick: (0.03, -0.03),
            ..Default::default()
        });
        assert_eq!((g.thumb_lx, g.thumb_ly), (983, -983));
    }

    #[test]
    fn rumble_plan_prefers_split_handles_then_falls_back() {
        use Locality::*;
        assert_eq!(plan_rumble(&[All, LeftHandle, RightHandle]), Some(RumblePlan::Split));
        assert_eq!(plan_rumble(&[Default, All, Handles]), Some(RumblePlan::Combined(Handles)));
        assert_eq!(plan_rumble(&[LeftHandle, All]), Some(RumblePlan::Combined(All)));
        assert_eq!(plan_rumble(&[Default]), Some(RumblePlan::Combined(Default)));
        assert_eq!(plan_rumble(&[]), None);
    }

    #[test]
    fn digital_buttons() {
        let g = gp(&Snapshot {
            menu: true,
            options: true,
            left_stick_press: true,
            dpad_up: true,
            dpad_right: true,
            ..Default::default()
        });
        let want = Buttons::START
            | Buttons::BACK
            | Buttons::LEFT_THUMB
            | Buttons::DPAD_UP
            | Buttons::DPAD_RIGHT;
        assert_eq!(g.buttons, want);
    }

    #[test]
    fn the_touchpad_passes_through_and_never_reaches_the_guest() {
        let s = Snapshot {
            touchpad: Some(RawTouchpad {
                click: true,
                primary: (0.5, -2.0),
                secondary: (0.0, 0.0),
            }),
            ..Default::default()
        };
        let st = map_snapshot(&s);
        assert_eq!(
            st.touchpad,
            Some(Touchpad {
                click: true,
                primary: Some((0.5, -1.0)),
                secondary: None,
            })
        );
        assert_eq!(map_snapshot(&Snapshot::default()).touchpad, None, "no touchpad");
        assert_eq!(gp(&s), gp(&Snapshot::default()), "the game has no touchpad");
        assert_eq!(touch_finger((f32::NAN, 0.2)), None);
        assert_eq!(touch_finger((0.0, 0.1)), Some((0.0, 0.1)));
    }

    #[test]
    fn hostile_values_do_not_poison_state() {
        let g = gp(&Snapshot {
            a: f32::NAN,
            b: 7.0,
            left_trigger: f32::INFINITY,
            ..Default::default()
        });
        assert_eq!(g.analog(AnalogButton::A), 0);
        assert_eq!(g.analog(AnalogButton::B), 255);
        assert_eq!(g.analog(AnalogButton::LeftTrigger), 0);
    }

    fn manager(src: &FakeSource) -> InputManager {
        let mut m = InputManager::new(ConvertConfig::default()).unwrap();
        m.add_backend(Box::new(GameControllerPads::with_source(src.clone())));
        m
    }

    #[test]
    fn four_ports_hotplug_and_masks() {
        let src = FakeSource::default();
        let mut m = manager(&src);
        m.sync();
        assert_eq!(m.device_changes(), (0, 0));
        src.0.lock().unwrap().pads = (1..=5).map(|k| raw(k, Snapshot::default())).collect();
        m.sync();
        assert_eq!(m.connected_mask(), 0b1111, "fifth pad has no port");
        assert_eq!(m.device_changes(), (0b1111, 0));
        // Pad in port 1 (key 2) vanishes; the waiting fifth pad takes the slot.
        src.0.lock().unwrap().pads.retain(|p| p.key != 2);
        m.sync();
        assert_eq!(m.device_changes(), (0b0010, 0b0010));
        assert_eq!(m.connected_mask(), 0b1111);
        // Pull everything.
        src.0.lock().unwrap().pads.clear();
        m.sync();
        assert_eq!(m.connected_mask(), 0);
        assert_eq!(m.device_changes(), (0, 0b1111));
    }

    #[test]
    fn replug_between_syncs_is_remove_plus_insert() {
        let src = FakeSource::default();
        let mut m = manager(&src);
        src.0.lock().unwrap().pads = vec![raw(1, Snapshot::default())];
        m.sync();
        m.device_changes();
        src.0.lock().unwrap().pads = vec![raw(2, Snapshot::default())]; // new connection key
        m.sync();
        assert_eq!(m.device_changes(), (1, 1));
    }

    #[test]
    fn state_reaches_the_guest_struct() {
        let src = FakeSource::default();
        let mut m = manager(&src);
        src.0.lock().unwrap().pads = vec![raw(
            1,
            Snapshot {
                a: 1.0,
                left_trigger: 1.0,
                left_stick: (1.0, 0.0),
                ..Default::default()
            },
        )];
        m.sync();
        let h = m.open(PortIndex::new(0).unwrap()).unwrap();
        let s = m.get_state(h).unwrap();
        assert_eq!(s.gamepad.analog(AnalogButton::A), 255);
        assert_eq!(s.gamepad.analog(AnalogButton::LeftTrigger), 255);
        assert_eq!(s.gamepad.thumb_lx, 32767);
    }

    #[test]
    fn rumble_is_forwarded_as_unit_speeds() {
        let src = FakeSource::default();
        let mut m = manager(&src);
        src.0.lock().unwrap().pads = vec![raw(9, Snapshot::default())];
        m.sync();
        let h = m.open(PortIndex::new(0).unwrap()).unwrap();
        m.set_state(
            h,
            FeedbackRequest {
                addr: 0,
                h_event: 0,
                rumble: Rumble {
                    left_motor: 65535,
                    right_motor: 0,
                },
            },
        )
        .unwrap();
        m.sync();
        assert_eq!(src.0.lock().unwrap().rumble, vec![(9, 1.0, 0.0)]);
    }

    #[test]
    fn lightbar_override_and_restore_are_forwarded_by_connection_key() {
        let src = FakeSource::default();
        let mut backend = GameControllerPads::with_source(src.clone());
        backend.set_lightbar(HostPadId(9), Some([17, 23, 29]));
        backend.set_lightbar(HostPadId(11), Some([31, 37, 41]));
        backend.set_lightbar(HostPadId(9), None);
        let log = src.0.lock().unwrap();
        assert_eq!(
            log.lightbar,
            vec![(9, Some([17, 23, 29])), (11, Some([31, 37, 41])), (9, None)]
        );
        assert!(log.rumble.is_empty());
    }

    #[test]
    fn lightbars_follow_the_ports_through_a_primary_pad_change() {
        use crate::backend::fallback::KeyboardFallback;
        use crate::backend::keyboard::KeyboardMouse;
        use crate::backend::multi::Multi;

        let source = FakeSource::default();
        source.0.lock().unwrap().pads =
            vec![raw(9, Snapshot::default()), raw(11, Snapshot::default())];
        let physical = Multi::new(vec![Box::new(GameControllerPads::with_source(source.clone()))]);
        let mut input = InputManager::new(Default::default()).unwrap();
        input.add_backend(Box::new(KeyboardFallback::new(
            KeyboardMouse::new(Default::default()),
            Some(Box::new(physical)),
            Default::default(),
        )));
        let take_output = || std::mem::take(&mut source.0.lock().unwrap().lightbar);
        input.sync();
        take_output();

        let lightbars = input.lightbars();
        let first = Some([19, 27, 33]);
        let second = Some([60, 40, 20]);
        let colours = [first, second, None, None];
        lightbars.update_game(true, colours);
        input.sync();
        input.sync();
        assert_eq!(take_output(), [(9, first), (11, second)]);

        source.0.lock().unwrap().pads.remove(0);
        input.sync();
        assert_eq!(take_output(), [(9, None), (11, first)]);

        source.0.lock().unwrap().pads.push(raw(13, Snapshot::default()));
        input.sync();
        assert_eq!(take_output(), [(13, second)]);

        lightbars.update_game(false, colours);
        input.sync();
        assert_eq!(take_output(), [(11, None), (13, None)]);
        lightbars.update_game(true, colours);
        input.sync();
        assert_eq!(take_output(), [(11, first), (13, second)]);
        input.restore_lightbars();
        assert_eq!(take_output(), [(11, None), (13, None)]);
    }
}
