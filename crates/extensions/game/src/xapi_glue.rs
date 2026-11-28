//! XAPI: crates/platform/xapi through `xapi::register` (docs/runtime/xapi.md "Boot
//! integration"), over `xapi::VfsStorage` on the kernel's VFS and a
//! headless InputManager with the keyboard pad. crates/platform/xapi owns the N:
//! cache drive at 0x2d5d0a. With `--no-xapi` the boot's old
//! XMountUtilityDrive override comes back.

use std::sync::{Arc, Mutex};

use anyhow::Context;

use crate::stopgaps::Uses;

pub type Shared = Arc<Mutex<xapi::Xapi<xapi::VfsStorage>>>;

/// Kernel event delivery for XAPI's input signals (no guard held).
pub struct KernelEvents(pub Arc<kernel::Kernel>);

impl xapi::Events for KernelEvents {
    fn signal(&mut self, guest_handle: u32) -> xapi::Result<()> {
        self.0.signal_event(guest_handle).map(|_| ()).map_err(|_| xapi::Error::Win32(6))
    }
}

/// After `k.set_vfs`, with no guard held. Also returns the keyboard pad, for
/// scripted input ([`InputScript`]).
#[cfg(feature = "desktop-runtime")]
pub fn register(rt: &runtime::Runtime, k: &Arc<kernel::Kernel>) -> anyhow::Result<(Shared, input::backend::keyboard::KeyboardMouse)> {
    let (input, keyboard) = input::InputManager::with_keyboard_pad(input::bindings::Bindings::default())
        .map_err(|e| anyhow::anyhow!("input: {e:?}"))?;
    Ok((register_with(rt, k, input)?, keyboard))
}

/// [`register`] on the caller's input manager (a host window's keyboard,
/// mouse and pads), instead of a headless keyboard pad.
pub fn register_with(rt: &runtime::Runtime, k: &Arc<kernel::Kernel>, input: input::InputManager) -> anyhow::Result<Shared> {
    let storage = xapi::VfsStorage::new(k.vfs().context("VFS not attached")?);
    let mut config = xapi::Config::for_gamepad(xapi::register::addr::XDEVICE_TYPE_GAMEPAD);
    // The communicator types answer, with no port until the media objects
    // are wired (docs/live/live-voice.md, 5): the same answers as before.
    config.devices.voice = Some(xapi::VoiceTypes::RETAIL);
    let x = xapi::Xapi::new(config, input, storage).map_err(|e| anyhow::anyhow!("xapi: {e}"))?;
    let x = Arc::new(Mutex::new(x));
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(xapi::register(d, &x, xapi::GuestLayout::default())));
    r.unwrap().map_err(|e| anyhow::anyhow!("xapi::register: {e}"))?;
    Ok(x)
}

/// A scripted pad timeline, fed through the keyboard pad and so through
/// XInputGetState. From `H2_BOOT_INPUT` (entries separated by `,`) and
/// `H2_BOOT_INPUT_FILE` (one entry per line, `#` comments), merged.
///
/// An entry is `BUTTONS@WHEN`. BUTTONS is one or more of `A B X Y BLACK
/// WHITE LT RT UP DOWN LEFT RIGHT START BACK LS RS LUP LDOWN LLEFT LRIGHT`
/// (the sticks deflect fully), or any keyboard key name, joined with `|`.
/// WHEN is `T`, `T+D` or `T-U`: a slice number, or a present count with an
/// `f` prefix (`f480+4`, `f480-f484`). The buttons go down once the time
/// reaches T and up at U (default: 4 presents, or 100 slices, later).
///
/// Example, title to main menu: `START@f300,DOWN@f420,A@f440`.
///
/// Holds combine: a physical input stays down while any entry holding it
/// (directly or through a binding) is active, whatever the entry order.
/// An endpoint past `u64::MAX` is a parse error.
///
/// Minimal on purpose: the input-full lane's scripted source replaces it.
#[cfg(feature = "desktop-runtime")]
pub struct InputScript(Vec<Entry>, Mutex<Vec<input::bindings::Input>>);

#[cfg(feature = "desktop-runtime")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    Slice(u64),
    Frame(u64),
}

#[cfg(feature = "desktop-runtime")]
impl When {
    fn reached(self, slice: u64, frame: u64) -> bool {
        match self {
            When::Slice(s) => slice >= s,
            When::Frame(f) => frame >= f,
        }
    }
}

#[cfg(feature = "desktop-runtime")]
#[derive(Debug)]
struct Entry {
    text: String,
    controls: Vec<Hold>,
    down: When,
    up: When,
    state: std::sync::atomic::AtomicU8,
}

#[cfg(feature = "desktop-runtime")]
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hold {
    Control(input::bindings::Control),
    Key(String),
}

#[cfg(feature = "desktop-runtime")]
fn control(name: &str) -> Option<input::bindings::Control> {
    use input::bindings::Control::*;
    Some(match name.to_ascii_uppercase().as_str() {
        "A" => A,
        "B" => B,
        "X" => X,
        "Y" => Y,
        "BLACK" => Black,
        "WHITE" => White,
        "LT" => LeftTrigger,
        "RT" => RightTrigger,
        "UP" => DpadUp,
        "DOWN" => DpadDown,
        "LEFT" => DpadLeft,
        "RIGHT" => DpadRight,
        "START" => Start,
        "BACK" => Back,
        "LS" => LeftThumb,
        "RS" => RightThumb,
        "LUP" => LeftStickUp,
        "LDOWN" => LeftStickDown,
        "LLEFT" => LeftStickLeft,
        "LRIGHT" => LeftStickRight,
        _ => return None,
    })
}

#[cfg(feature = "desktop-runtime")]
fn parse_when(t: &str, frame_default: bool) -> Option<When> {
    let t = t.trim();
    match t.strip_prefix('f') {
        Some(n) => n.parse().ok().map(When::Frame),
        None if frame_default => t.parse().ok().map(When::Frame),
        None => t.parse().ok().map(When::Slice),
    }
}

#[cfg(feature = "desktop-runtime")]
fn parse_entry(item: &str) -> Option<Entry> {
    let (buttons, when) = item.trim().split_once('@')?;
    let controls = buttons.split('|').map(|b| control(b.trim()).map_or_else(|| Hold::Key(b.trim().to_string()), Hold::Control)).collect();
    let later = |down: When, d: u64| match down {
        When::Slice(s) => s.checked_add(d).map(When::Slice),
        When::Frame(f) => f.checked_add(d).map(When::Frame),
    };
    let (down, up) = if let Some((a, d)) = when.split_once('+') {
        let down = parse_when(a, false)?;
        (down, later(down, d.trim().parse().ok()?)?)
    } else if let Some((a, b)) = when.split_once('-') {
        let down = parse_when(a, false)?;
        (down, parse_when(b, matches!(down, When::Frame(_)))?)
    } else {
        let down = parse_when(when, false)?;
        (down, later(down, if matches!(down, When::Frame(_)) { 4 } else { 100 })?)
    };
    Some(Entry { text: item.trim().to_string(), controls, down, up, state: Default::default() })
}

#[cfg(feature = "desktop-runtime")]
impl InputScript {
    /// Parse a timeline; entries separated by `,` or newlines, `#` starts
    /// a comment. Unparseable entries are reported and skipped.
    pub fn parse(spec: &str) -> Self {
        let mut entries = Vec::new();
        for line in spec.lines() {
            let line = line.split('#').next().unwrap_or("");
            for item in line.split(',').filter(|i| !i.trim().is_empty()) {
                match parse_entry(item) {
                    Some(e) => entries.push(e),
                    None => crate::log::note(&format!("input: cannot parse `{}`", item.trim())),
                }
            }
        }
        InputScript(entries, Mutex::new(Vec::new()))
    }

    pub fn from_env() -> Self {
        let mut spec = std::env::var("H2_BOOT_INPUT").unwrap_or_default();
        if let Ok(path) = std::env::var("H2_BOOT_INPUT_FILE") {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    spec.push('\n');
                    spec.push_str(&text);
                }
                Err(e) => crate::log::note(&format!("input: {path}: {e}")),
            }
        }
        let script = Self::parse(&spec);
        if !script.is_empty() {
            let entries: Vec<&str> = script.0.iter().map(|e| e.text.as_str()).collect();
            eprintln!("input: script of {} entries from H2_BOOT_INPUT/H2_BOOT_INPUT_FILE: {}", entries.len(), entries.join(", "));
        }
        script
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The physical input a hold presses: a control's first binding, or
    /// the key (canonical, so `Space` and `space` are one input).
    fn physical(pad: &input::backend::keyboard::KeyboardMouse, hold: &Hold) -> Option<input::bindings::Input> {
        let i = match hold {
            Hold::Control(c) => pad.bindings().inputs(*c).first().cloned(),
            Hold::Key(k) => Some(input::bindings::Input::Key(input::bindings::canonical_key(k).unwrap_or_else(|| k.clone()))),
        };
        if i.is_none() {
            crate::log::note(&format!("input: {hold:?} has no keyboard binding"));
        }
        i
    }

    fn press(pad: &input::backend::keyboard::KeyboardMouse, i: &input::bindings::Input, down: bool) {
        match i {
            input::bindings::Input::Key(k) if down => drop(pad.key_down(k)),
            input::bindings::Input::Key(k) => drop(pad.key_up(k)),
            input::bindings::Input::Mouse(b) => pad.mouse_button(*b, down),
        }
    }

    /// Apply the script before `slice`, at the current present count.
    pub fn step(&self, slice: u64, pad: &input::backend::keyboard::KeyboardMouse) {
        self.step_at(slice, crate::presents::presents(), pad);
    }

    fn step_at(&self, slice: u64, frame: u64, pad: &input::backend::keyboard::KeyboardMouse) {
        use std::sync::atomic::Ordering::Relaxed;
        let mut want: Vec<input::bindings::Input> = Vec::new();
        for e in &self.0 {
            let st = e.state.load(Relaxed);
            if st == 0 && e.down.reached(slice, frame) {
                e.state.store(1, Relaxed);
                let keys: Vec<String> = e.controls.iter().filter_map(|h| Self::physical(pad, h)).map(|i| format!("{i:?}")).collect();
                eprintln!("input: `{}` down at slice {slice}, present {frame} (keys {})", e.text, keys.join(" "));
            } else if st == 1 && e.up.reached(slice, frame) {
                e.state.store(2, Relaxed);
                eprintln!("input: `{}` up at slice {slice}, present {frame}", e.text);
            }
            if e.state.load(Relaxed) == 1 {
                for i in e.controls.iter().filter_map(|h| Self::physical(pad, h)) {
                    if !want.contains(&i) {
                        want.push(i);
                    }
                }
            }
        }
        // Transitions from the previous aggregate: overlapping, adjacent and
        // aliased holds of one input never release it early.
        let mut held = self.1.lock().unwrap();
        for i in held.iter().filter(|i| !want.contains(i)) {
            Self::press(pad, i, false);
        }
        for i in want.iter().filter(|i| !held.contains(i)) {
            Self::press(pad, i, true);
        }
        *held = want;
    }
}

/// `BOOL XMountUtilityDrive(BOOL fFormatClean)` at 0x002d5b77 (docs/runtime/xapi.md),
/// only with `--no-xapi`: the retail body selects a cache partition by
/// reading the raw disk. Native policy: Z: is the configured utility
/// partition; a format request is ignored.
pub const X_MOUNT_UTILITY_DRIVE: u32 = 0x002d_5b77;

pub fn install_no_xapi_mount(rt: &runtime::Runtime, k: &Arc<kernel::Kernel>, utility_partition: u8, uses: &Uses) {
    let k = Arc::clone(k);
    let u = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(X_MOUNT_UTILITY_DRIVE, "boot:XMountUtilityDrive", move |cpu, ctx, _| {
            let format = ctx.runtime().memory().u32(cpu.esp + 4)?;
            u.lock().unwrap().entry(0x2d5b77).or_insert(("XMountUtilityDrive (xapi)", 0)).1 += 1;
            let ok = k.vfs().is_some_and(|v| {
                let mut v = v.lock().unwrap();
                let _ = v.namespace.delete_symbolic_link("\\??\\Z:");
                v.namespace
                    .create_symbolic_link("\\??\\Z:", &format!("\\Device\\Harddisk0\\Partition{utility_partition}"))
                    .is_ok()
            });
            if format != 0 {
                crate::log::note("XMountUtilityDrive(fFormatClean=TRUE): format ignored");
            }
            cpu.eax = ok as u32;
            let mut mem = ctx.runtime().memory();
            // The retail body records the chosen cache-DB slot (partition - 3)
            // at 0x461d80; later XAPI cache code (0x2d5d0a) requires it.
            mem.set_u32(xapi::register::addr::CACHE_ROW, utility_partition as u32 - 3)?;
            mem.ret(cpu, 4)
        });
    });
}

#[cfg(all(test, feature = "desktop-runtime"))]
mod tests {
    use super::*;

    #[test]
    fn timeline_entries() {
        let s = InputScript::parse("START@f300, Escape@10000-10100\n# comment\nDOWN|A@f420+2,LUP@5\nnonsense");
        let w: Vec<_> = s.0.iter().map(|e| (e.controls.clone(), e.down, e.up)).collect();
        use input::bindings::Control::*;
        assert_eq!(
            w,
            [
                (vec![Hold::Control(Start)], When::Frame(300), When::Frame(304)),
                (vec![Hold::Key("Escape".into())], When::Slice(10000), When::Slice(10100)),
                (vec![Hold::Control(DpadDown), Hold::Control(A)], When::Frame(420), When::Frame(422)),
                (vec![Hold::Control(LeftStickUp)], When::Slice(5), When::Slice(105)),
            ]
        );
        assert_eq!(parse_entry("A@f10-12").map(|e| e.up), Some(When::Frame(12)));
    }

    #[test]
    fn overflowing_endpoints_are_parse_errors() {
        for bad in ["A@18446744073709551615", "A@f18446744073709551615", "A@18446744073709551600+100", "A@f5+18446744073709551615"] {
            assert!(parse_entry(bad).is_none(), "{bad}");
        }
        assert!(InputScript::parse("A@18446744073709551615").is_empty());
        assert!(parse_entry("A@18446744073709551515").is_some());
    }

    /// Slices 0..20: whether A is down before each.
    fn a_timeline(spec: &str) -> Vec<bool> {
        let pad = input::backend::keyboard::KeyboardMouse::new(input::bindings::Bindings::default());
        let idle = format!("{:?}", pad.state());
        let s = InputScript::parse(spec);
        (0..20)
            .map(|slice| {
                s.step_at(slice, 0, &pad);
                format!("{:?}", pad.state()) != idle
            })
            .collect()
    }

    fn held(from: u64, to: u64) -> Vec<bool> {
        (0..20).map(|t| (from..to).contains(&t)).collect()
    }

    #[test]
    fn overlapping_holds_keep_the_button_down() {
        assert_eq!(a_timeline("A@0-10,A@5-15"), held(0, 15));
        assert_eq!(a_timeline("A@5-15,A@0-10"), held(0, 15));
        assert_eq!(a_timeline("A@0-15,A@5-10"), held(0, 15));
    }

    #[test]
    fn adjacent_holds_do_not_depend_on_entry_order() {
        assert_eq!(a_timeline("A@0-5,A@5-10"), held(0, 10));
        assert_eq!(a_timeline("A@5-10,A@0-5"), held(0, 10));
    }

    #[test]
    fn aliased_holds_share_one_key() {
        let pad = input::backend::keyboard::KeyboardMouse::new(input::bindings::Bindings::default());
        let Some(input::bindings::Input::Key(k)) = InputScript::physical(&pad, &Hold::Control(input::bindings::Control::A)) else {
            panic!("A is bound to a key")
        };
        let upper = k.to_ascii_uppercase();
        assert_eq!(a_timeline(&format!("A@0-10,{upper}@5-15")), held(0, 15));
        assert_eq!(a_timeline(&format!("{upper}@5-15,A@0-10")), held(0, 15));
    }

    #[test]
    fn presses_reach_the_pad() {
        let pad = input::backend::keyboard::KeyboardMouse::new(input::bindings::Bindings::default());
        let s = InputScript::parse("A@3-5");
        s.step(2, &pad);
        let idle = pad.state();
        s.step(3, &pad);
        assert_ne!(format!("{:?}", pad.state()), format!("{idle:?}"));
        s.step(5, &pad);
        assert_eq!(format!("{:?}", pad.state()), format!("{idle:?}"));
    }
}
