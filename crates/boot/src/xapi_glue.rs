//! XAPI: crates/xapi through `xapi::register` (docs/xapi.md "Boot
//! integration"), over `xapi::VfsStorage` on the kernel's VFS and a
//! headless InputManager with the keyboard pad. crates/xapi owns the N:
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
pub fn register(rt: &runtime::Runtime, k: &Arc<kernel::Kernel>) -> anyhow::Result<(Shared, input::backend::keyboard::KeyboardMouse)> {
    let (input, keyboard) = input::InputManager::with_keyboard_pad(input::bindings::Bindings::default())
        .map_err(|e| anyhow::anyhow!("input: {e:?}"))?;
    let storage = xapi::VfsStorage::new(k.vfs().context("VFS not attached")?);
    let config = xapi::Config::for_gamepad(xapi::register::addr::XDEVICE_TYPE_GAMEPAD);
    let x = xapi::Xapi::new(config, input, storage).map_err(|e| anyhow::anyhow!("xapi: {e}"))?;
    let x = Arc::new(Mutex::new(x));
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(xapi::register(d, &x, xapi::GuestLayout::default())));
    r.unwrap().map_err(|e| anyhow::anyhow!("xapi::register: {e}"))?;
    Ok((x, keyboard))
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
/// Minimal on purpose: the input-full lane's scripted source replaces it.
pub struct InputScript(Vec<Entry>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum When {
    Slice(u64),
    Frame(u64),
}

impl When {
    fn reached(self, slice: u64, frame: u64) -> bool {
        match self {
            When::Slice(s) => slice >= s,
            When::Frame(f) => frame >= f,
        }
    }
}

#[derive(Debug)]
struct Entry {
    text: String,
    controls: Vec<Hold>,
    down: When,
    up: When,
    state: std::sync::atomic::AtomicU8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Hold {
    Control(input::bindings::Control),
    Key(String),
}

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

fn parse_when(t: &str, frame_default: bool) -> Option<When> {
    let t = t.trim();
    match t.strip_prefix('f') {
        Some(n) => n.parse().ok().map(When::Frame),
        None if frame_default => t.parse().ok().map(When::Frame),
        None => t.parse().ok().map(When::Slice),
    }
}

fn parse_entry(item: &str) -> Option<Entry> {
    let (buttons, when) = item.trim().split_once('@')?;
    let controls = buttons.split('|').map(|b| control(b.trim()).map_or_else(|| Hold::Key(b.trim().to_string()), Hold::Control)).collect();
    let (down, up) = if let Some((a, d)) = when.split_once('+') {
        let down = parse_when(a, false)?;
        let d: u64 = d.trim().parse().ok()?;
        let up = match down {
            When::Slice(s) => When::Slice(s + d),
            When::Frame(f) => When::Frame(f + d),
        };
        (down, up)
    } else if let Some((a, b)) = when.split_once('-') {
        let down = parse_when(a, false)?;
        (down, parse_when(b, matches!(down, When::Frame(_)))?)
    } else {
        let down = parse_when(when, false)?;
        let up = match down {
            When::Slice(s) => When::Slice(s + 100),
            When::Frame(f) => When::Frame(f + 4),
        };
        (down, up)
    };
    Some(Entry { text: item.trim().to_string(), controls, down, up, state: Default::default() })
}

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
        InputScript(entries)
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
        Self::parse(&spec)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn press(pad: &input::backend::keyboard::KeyboardMouse, hold: &Hold, down: bool) {
        let inputs = match hold {
            Hold::Control(c) => pad.bindings().inputs(*c).to_vec(),
            Hold::Key(k) => vec![input::bindings::Input::Key(k.clone())],
        };
        let Some(i) = inputs.first() else {
            crate::log::note(&format!("input: {hold:?} has no keyboard binding"));
            return;
        };
        match i {
            input::bindings::Input::Key(k) if down => drop(pad.key_down(k)),
            input::bindings::Input::Key(k) => drop(pad.key_up(k)),
            input::bindings::Input::Mouse(b) => pad.mouse_button(*b, down),
        }
    }

    /// Apply the script before `slice`, at the current present count.
    pub fn step(&self, slice: u64, pad: &input::backend::keyboard::KeyboardMouse) {
        use std::sync::atomic::Ordering::Relaxed;
        let frame = crate::d3d_diag::presents();
        for e in &self.0 {
            let st = e.state.load(Relaxed);
            if st == 0 && e.down.reached(slice, frame) {
                e.controls.iter().for_each(|h| Self::press(pad, h, true));
                e.state.store(1, Relaxed);
                eprintln!("input: `{}` down at slice {slice}, present {frame}", e.text);
            } else if st == 1 && e.up.reached(slice, frame) {
                e.controls.iter().for_each(|h| Self::press(pad, h, false));
                e.state.store(2, Relaxed);
            }
        }
    }
}

/// `BOOL XMountUtilityDrive(BOOL fFormatClean)` at 0x002d5b77 (docs/xapi.md),
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

#[cfg(test)]
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
