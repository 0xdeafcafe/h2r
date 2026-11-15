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

/// `H2_BOOT_INPUT=key@from-to[,...]`: hold a keyboard-pad key (a binding
/// name such as `Escape` for START, `Space` for A, `ArrowDown`) from slice `from` up
/// to slice `to`.
pub struct InputScript(Vec<(String, u64, u64)>);

impl InputScript {
    pub fn from_env() -> Self {
        let spec = std::env::var("H2_BOOT_INPUT").unwrap_or_default();
        InputScript(
            spec.split(',')
                .filter_map(|item| {
                    let (key, range) = item.trim().split_once('@')?;
                    let (a, b) = range.split_once('-')?;
                    Some((key.to_string(), a.parse().ok()?, b.parse().ok()?))
                })
                .collect(),
        )
    }

    /// Apply the script at the start of `slice`.
    pub fn step(&self, slice: u64, pad: &input::backend::keyboard::KeyboardMouse) {
        for (key, from, to) in &self.0 {
            if slice == *from {
                pad.key_down(key);
                crate::log::note(&format!("input: {key} down at slice {slice}"));
            }
            if slice == *to {
                pad.key_up(key);
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
