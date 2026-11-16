//! Retail virtual keyboard bridge. The host queues text; only the guest thread
//! inspects screens and calls the game's own edit/validation handlers.
use input::backend::keyboard::{KeyboardMouse, TextAction};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, Runtime, TransferAbi, guest::Permissions};
use std::{collections::VecDeque, sync::Arc};

/// Per-frame UI update (one float argument, ret 4), called by 0x12b450.
pub const PUMP: u32 = 0x0014_79c3;
pub const FACTORY: u32 = 0x0023_784f;
pub const VTABLE: u32 = 0x0045_9ba0;
pub const SCREEN_ID: u32 = 0xa7;
pub const CHANNEL: u32 = 0x0054_dba8;
pub const INSERT: u32 = 0x0023_8932;
pub const BACKSPACE: u32 = 0x0023_8a05;
pub const KEY_ACTION: u32 = 0x0023_82d6;
pub const EVENT: u32 = 0x0023_7f2a;
pub const BUFFER: u32 = 0x818;
pub const CARET: u32 = 0x81c;
pub const CAPACITY: u32 = 0x820;

/// A validated focused screen. Capacity includes the UTF-16 terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub address: u32,
    pub buffer: u32,
    pub caret: u32,
    pub capacity: u16,
    pub context: u32,
    pub controller: u32,
}
impl Screen {
    pub fn target(self) -> u64 {
        (u64::from(self.address) << 32) | u64::from(self.buffer)
    }
}

/// The factory puts keyboards on shared channel 2, window 4. Global modal
/// channel 0 and shared channel 1 have input priority (0x147dbe). Never type
/// through a validation dialog or during an outgoing transition.
pub fn detect(m: &Guest) -> Option<Screen> {
    for channel in [0x0054_d99c, 0x0054_d964] {
        if m.u32(channel + 8).ok()? != 0 || m.u32(channel + 12).ok()? != 0 {
            return None;
        }
    }
    let address = m.u32(CHANNEL + 8).ok()?;
    if address == 0 || m.u32(CHANNEL + 12).ok()? != 0 {
        return None;
    }
    let at = |offset| address.checked_add(offset);
    if m.u32(address).ok()? != VTABLE
        || m.u32(at(4)?).ok()? != 0
        || m.u32(at(0x70)?).ok()? != SCREEN_ID
        || m.u16(at(8)?).ok()? & 0xf == 0
        || m.u8(at(0x42)?).ok()? & 2 != 0
    {
        return None;
    }
    let context = m.u32(at(0x610)?).ok()?;
    let buffer = m.u32(at(BUFFER)?).ok()?;
    let caret = m.u32(at(CARET)?).ok()?;
    let capacity = m.u16(at(CAPACITY)?).ok()?;
    if (context == 14 && m.u32(at(0x372c)?).ok()? != u32::MAX)
        || context > 20
        || buffer == 0
        || buffer & 1 != 0
        || !(1..=150).contains(&capacity)
    {
        return None;
    }
    m.check(
        buffer,
        usize::from(capacity) * 2,
        runtime::guest::Access::Write,
        1,
    )
    .ok()?;
    let len = (0..u32::from(capacity)).find(|i| m.u16(buffer + i * 2).ok() == Some(0))?;
    if caret < buffer || caret > buffer.checked_add(len * 2)? || (caret - buffer) & 1 != 0 {
        return None;
    }
    Some(Screen {
        address,
        buffer,
        caret,
        capacity,
        context,
        controller: (m.u16(at(8)?).ok()? & 0xf).trailing_zeros(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Insert(u16),
    Backspace,
    Accept,
    Cancel,
}
fn edits(events: Vec<TextAction>) -> VecDeque<Edit> {
    let mut out = VecDeque::new();
    for event in events {
        match event {
            TextAction::Insert(s) => out.extend(
                s.chars()
                    .filter(|c| !c.is_control() && u32::from(*c) <= 0xffff)
                    .map(|c| {
                        // Match the on-screen key handler's 0x140420 filter.
                        let unit = c as u16;
                        Edit::Insert(if (0xe000..=0xf8ff).contains(&unit) {
                            0x25a1
                        } else {
                            unit
                        })
                    }),
            ),
            TextAction::Backspace => out.push_back(Edit::Backspace),
            TextAction::Accept => {
                out.push_back(Edit::Accept);
                break;
            }
            TextAction::Cancel => {
                out.push_back(Edit::Cancel);
                break;
            }
        }
    }
    out
}

fn finish(cpu: &mut CpuAbi, saved: CpuAbi, return_pc: u32) -> TransferAbi {
    *cpu = saved;
    TransferAbi::ret(return_pc)
}
fn apply_next(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    keyboard: KeyboardMouse,
    screen: Screen,
    mut pending: VecDeque<Edit>,
    saved: CpuAbi,
    return_pc: u32,
) -> Result<TransferAbi, GuestFault> {
    let current = detect(&ctx.runtime().memory());
    if current.is_none_or(|s| s.target() != screen.target()) {
        keyboard.set_text_target(current.map(Screen::target));
        return Ok(finish(cpu, saved, return_pc));
    }
    let Some(edit) = pending.pop_front() else {
        return Ok(finish(cpu, saved, return_pc));
    };
    *cpu = saved;
    let rt = Arc::clone(ctx.runtime());
    let mut m = rt.memory();
    let target = match edit {
        Edit::Insert(unit) => {
            cpu.eax = screen.address;
            cpu.esp -= 4;
            m.set_u32(cpu.esp, u32::from(unit))?;
            INSERT
        }
        Edit::Backspace => {
            cpu.eax = screen.address;
            BACKSPACE
        }
        Edit::Accept => {
            keyboard.set_text_target(None);
            cpu.ecx = screen.address;
            cpu.eax = 0x28;
            KEY_ACTION
        }
        Edit::Cancel => {
            keyboard.set_text_target(None);
            cpu.esp -= 16;
            let event = cpu.esp;
            m.write_bytes(event, &[0; 16], 1)?;
            m.set_u32(event, 5)?;
            m.set_u32(event + 4, screen.controller)?;
            m.set_u32(event + 8, 1)?;
            cpu.ecx = screen.address;
            cpu.esp -= 4;
            m.set_u32(cpu.esp, event)?;
            EVENT
        }
    };
    ctx.call_guest(cpu, &mut m, target, move |cpu, ctx| {
        // A full field need not make a native error sound for every remaining
        // character of a paste. Later Backspace/Done actions still run.
        if matches!(edit, Edit::Insert(_)) && cpu.eax & 0xff == 0 {
            while matches!(pending.front(), Some(Edit::Insert(_))) {
                pending.pop_front();
            }
        }
        apply_next(cpu, ctx, keyboard, screen, pending, saved, return_pc)
    })
}

pub fn install(rt: &Runtime, keyboard: KeyboardMouse) -> anyhow::Result<bool> {
    install_at(rt, keyboard, PUMP)
}
/// Chosen pump for synthetic native fixtures; production uses PUMP.
pub fn install_at(rt: &Runtime, keyboard: KeyboardMouse, pump: u32) -> anyhow::Result<bool> {
    let Some(original) = rt.resolve_original(pump) else {
        keyboard.set_text_target(None);
        eprintln!("keyboard: no original UI frame entry at {pump:#x}; hook skipped");
        return Ok(false);
    };
    if let Some(name) = rt.edit_dispatch(|d| d.override_name(pump).map(str::to_owned)) {
        anyhow::bail!("keyboard: UI pump already overridden by {name}");
    }
    let shadow = {
        let mut m = rt.memory();
        let mut base = 0xa2f2_0000u32;
        while m.mappings().iter().any(|r| {
            (base as u64) < u64::from(r.virtual_start) + u64::from(r.len)
                && base + 0x1000 > r.virtual_start
        }) || m.virtual_claims().iter().any(|r| {
            (base as u64) < u64::from(r.virtual_start) + u64::from(r.len)
                && base + 0x1000 > r.virtual_start
        }) {
            base = base
                .checked_add(0x1000)
                .filter(|v| *v < 0xa300_0000)
                .ok_or_else(|| anyhow::anyhow!("keyboard: no trampoline page"))?;
        }
        m.map_zeroed(base, 0x1000, Permissions::RX)?;
        m.claim_virtual(base, 0x1000, "host keyboard trampoline")?;
        base
    };
    rt.edit_dispatch(|d| {
        d.register_override(shadow, "keyboard: original UI frame", move |cpu, ctx, _| {
            Ok(original.invoke(cpu, ctx, pump))
        });
        d.register_override(pump, "keyboard: host text", move |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let mut m = rt.memory();
            let return_pc = m.u32(cpu.esp)?;
            cpu.esp += 4;
            let keyboard = keyboard.clone();
            ctx.call_guest(cpu, &mut m, shadow, move |cpu, ctx| {
                let screen = detect(&ctx.runtime().memory());
                keyboard.set_text_target(screen.map(Screen::target));
                let Some(screen) = screen else {
                    return Ok(TransferAbi::ret(return_pc));
                };
                let pending = edits(keyboard.take_text(screen.target()));
                let saved = *cpu;
                apply_next(cpu, ctx, keyboard, screen, pending, saved, return_pc)
            })
        });
    });
    Ok(true)
}
