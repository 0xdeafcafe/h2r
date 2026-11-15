//! In-game button layouts: Bumper Jumper, Recon and Walkie Talkie as extra
//! rows on Halo 2's own Settings, Controls, Button Layout screen.
//!
//! The design, addresses and what is still unverified are in
//! docs/controller-layouts.md. In short: the profile's layout byte selects a
//! 16-slot action table at `0x190c34`; the game treats a byte it does not know
//! as Default. This module (1) rewrites the table after the original runs for
//! bytes 4 to 6, (2) grows the screen's list from four rows to seven and
//! teaches its row text, selection and initial row about the added rows, (3)
//! serves our own English text through the locale lookup, and (4) gives the
//! screen's tag three more panes per thumbstick diagram, cloned from the
//! default one with the labels moved to where the layout puts the actions.
//!
//! Every hook is a wrapper at a function entry. Rows and bytes 0 to 3 go to
//! the generated original untouched. Set `H2_NO_LAYOUTS=1` to skip all of it.
//! The logic is written over the small [`Gm`] trait so it is tested without
//! a guest.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use input::layouts::{Action, Button, Layout};
use runtime::dispatch::Dispatch;
use runtime::exec::OriginalHandler;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

// ---- guest addresses (hook points; see docs/controller-layouts.md) ----

const ACTION_TABLE_BUILD: u32 = 0x0019_0c34;
const LIST_DATA_NEW: u32 = 0x001a_480d;
const ROW_TEXT: u32 = 0x002c_6512;
const ROW_SELECTED: u32 = 0x002c_6567;
const INITIAL_ROW: u32 = 0x002b_463d;
const PANE_SET: u32 = 0x0023_0427;
const SCREEN_TAG: u32 = 0x0022_f871;
const TEXT_SET: u32 = 0x0025_3b1a;
const LOCALE_FIND: u32 = 0x001a_02d0;
const PROFILE_ROWS: u32 = 0x0024_a886;
const CONTROLS_ROW_TEXT: u32 = 0x002c_8548;
const TEXT_WIDGET_GET: u32 = 0x0022_edb8;
const LIST_SELECT: u32 = 0x0024_c591;
const SCREEN_REFRESH: u32 = 0x0022_f672;

/// Per-player profile settings (`+4` layout byte) and the action tables.
const SETTINGS_BASE: u32 = 0x0054_e9f4;
const SETTINGS_STRIDE: u32 = 0xc70;
const TABLE_BASE: u32 = 0x0051_ea18;
const TABLE_STRIDE: u32 = 0x1c;
const TABLE_SLOTS: u32 = 8;
/// The Controls edit buffer's layout byte.
const EDIT_LAYOUT: u32 = 0x0054_e6d8;
/// Name of the list's item array, which marks it as the layout list.
const LIST_NAME: u32 = 0x0045_ce18;
const VANILLA_ROWS: u32 = 4;
const SCREEN_ID: i16 = 40;
const VANILLA_PANES: u32 = 8;
const PANE_SIZE: u32 = 0x4c;
const TEXT_SIZE: u32 = 0x2c;
/// Offset of the edit-profile screen's profile copy and the layout byte in it.
const PROFILE_COPY_LAYOUT: u32 = 0x176c + 0x104;

/// Our guest memory: strings, then the new pane tables.
const DATA_BASE: u32 = 0xa0ff_8000;
const DATA_SIZE: u32 = 0x1_0000;
const SHADOW_BASE: u32 = 0xa0ff_6000;

pub const ADDED: [Layout; 3] = [Layout::BumperJumper, Layout::Recon, Layout::WalkieTalkie];

// ---- guest memory seen by the logic ----

pub trait Gm {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool;
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool;
    fn u8(&self, a: u32) -> Option<u8> {
        let mut b = [0; 1];
        self.rd(a, &mut b).then_some(b[0])
    }
    fn u16(&self, a: u32) -> Option<u16> {
        let mut b = [0; 2];
        self.rd(a, &mut b).then_some(u16::from_le_bytes(b))
    }
    fn u32(&self, a: u32) -> Option<u32> {
        let mut b = [0; 4];
        self.rd(a, &mut b).then_some(u32::from_le_bytes(b))
    }
    fn set_u16(&mut self, a: u32, v: u16) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
    fn set_u32(&mut self, a: u32, v: u32) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
}

impl Gm for Guest {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.read_bytes(addr, out, 1).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.write_bytes(addr, data, 1).is_ok()
    }
}

// ---- pure logic ----

/// The layout for a row of the list, if it is one of ours.
pub fn layout_of_row(row: i16) -> Option<Layout> {
    let i = i32::from(row) - VANILLA_ROWS as i32;
    usize::try_from(i).ok().and_then(|i| ADDED.get(i)).copied()
}

/// The list's item count: ours for the layout list, unchanged otherwise.
pub fn list_count(name: u32, count: u32) -> u32 {
    if name == LIST_NAME && count == VANILLA_ROWS { count + ADDED.len() as u32 } else { count }
}

/// After the original built player `player`'s action table: overwrite it for
/// an added layout. Retail bytes (and bytes nothing knows) are left alone.
pub fn apply_action_table<M: Gm>(m: &mut M, player: u32) -> bool {
    let Some(byte) = m.u8(SETTINGS_BASE + SETTINGS_STRIDE.wrapping_mul(player) + 4) else { return false };
    match Layout::from_preset(byte) {
        Some(l) if !l.is_retail() => m.wr(TABLE_BASE + TABLE_STRIDE.wrapping_mul(player) + TABLE_SLOTS, &l.action_map()),
        _ => false,
    }
}

/// The pane to show for list row `row` when the screen's pane base is `base`
/// (0, or 4 for the Legacy thumbstick diagram): the row value to hand the
/// game so that `base + value` lands on our pane. `None` leaves it alone.
pub fn pane_row(row: i16, base: i8) -> Option<i16> {
    let l = layout_of_row(row)?;
    let slot = i16::from(l.preset()) - 4;
    let group = i16::from(base != 0);
    Some(VANILLA_PANES as i16 + slot + ADDED.len() as i16 * group - i16::from(base))
}

/// Our strings. Ids live in an index range no real string uses.
pub const ID_BASE: u32 = 0x20ff_f000;
pub fn is_ours(id: u32) -> bool {
    id & 0x00ff_ff00 == 0x00ff_f000
}
pub fn name_id(l: Layout) -> u32 {
    ID_BASE + (u32::from(l.preset()) - 4)
}
pub fn help_id(l: Layout) -> u32 {
    ID_BASE + 0x10 + (u32::from(l.preset()) - 4)
}
pub fn name_text(l: Layout) -> &'static str {
    match l {
        Layout::BumperJumper => "Bumper Jumper",
        Layout::Recon => "Recon",
        _ => "Walkie Talkie",
    }
}
pub fn help_text(l: Layout) -> &'static str {
    match l {
        Layout::BumperJumper => "Jump on the left bumper, melee on the right. Grenades on A, reload on B, flashlight on X.",
        Layout::Recon => "Reload and action on the right bumper. Switch grenades moves to X.",
        _ => "Melee on the right bumper, reload on B, switch grenades on X.",
    }
}

/// Every string we serve, as `(id, text)`.
pub fn strings() -> Vec<(u32, &'static str)> {
    ADDED.iter().flat_map(|l| [(name_id(*l), name_text(*l)), (help_id(*l), help_text(*l))]).collect()
}

/// Write the strings (NUL-terminated UTF-8, as the locale blob stores them)
/// at `base`; returns the address of each id.
pub fn write_strings<M: Gm>(m: &mut M, base: u32) -> Option<Vec<(u32, u32)>> {
    let mut at = base;
    let mut out = Vec::new();
    for (id, text) in strings() {
        let mut b = text.as_bytes().to_vec();
        b.push(0);
        if !m.wr(at, &b) {
            return None;
        }
        out.push((id, at));
        at += b.len() as u32;
    }
    Some(out)
}

/// Text block order of the default pane: the help text, then twelve labels.
const PANE_TEXT_ORDER: [Option<Action>; 13] = [
    None,
    Some(Action::Grenade),
    Some(Action::Crouch),
    Some(Action::Score),
    Some(Action::Pause),
    Some(Action::Fire),
    Some(Action::ActionReload),
    Some(Action::SwitchWeapon),
    Some(Action::Melee),
    Some(Action::Jump),
    Some(Action::SwitchGrenades),
    Some(Action::Flashlight),
    Some(Action::Zoom),
];

/// The label a diagram slot shows. A slot sits at one physical button. In
/// the default layout that button drives `slot_action`; in `l` it drives
/// whichever action `l` puts there.
pub fn relabel(l: Layout, slot_action: Action) -> Action {
    let button: Button = Layout::Default.button(slot_action);
    Action::GAMEPLAY.into_iter().find(|a| l.button(*a) == button).unwrap_or(slot_action)
}

fn text_block<M: Gm>(m: &M, pane: u32) -> Option<(u32, u32)> {
    Some((m.u32(pane + 0x1c)?, m.u32(pane + 0x20)?))
}

/// Grow the tag's pane array from 8 to 14: for each thumbstick diagram (pane
/// 0 and pane 4), one clone per added layout with its labels moved and its
/// help text replaced. `alloc` is where the new tables go. Does nothing (and
/// reports `Ok(false)`) unless the tag has exactly the retail 8 panes, so it
/// is safe to call again.
pub fn extend_panes<M: Gm>(m: &mut M, wgit: u32, alloc: &mut u32) -> Option<bool> {
    if m.u16(wgit + 4)? as i16 != SCREEN_ID || m.u32(wgit + 0x20)? != VANILLA_PANES {
        return Some(false);
    }
    let old = m.u32(wgit + 0x24)?;
    // Label sids by action, from the default pane's fixed text order.
    let (n, t0) = text_block(m, old)?;
    if n as usize != PANE_TEXT_ORDER.len() {
        return Some(false);
    }
    let mut label = std::collections::HashMap::new();
    for (i, a) in PANE_TEXT_ORDER.iter().enumerate() {
        if let Some(a) = a {
            label.insert(*a, m.u32(t0 + i as u32 * TEXT_SIZE + 0x24)?);
        }
    }
    let sid_action: std::collections::HashMap<u32, Action> = label.iter().map(|(a, s)| (*s, *a)).collect();

    let total = VANILLA_PANES + 2 * ADDED.len() as u32;
    let panes = *alloc;
    *alloc += total * PANE_SIZE;
    let mut buf = vec![0; (VANILLA_PANES * PANE_SIZE) as usize];
    if !m.rd(old, &mut buf) || !m.wr(panes, &buf) {
        return None;
    }
    for group in 0..2u32 {
        let template = old + group * 4 * PANE_SIZE;
        let (cnt, texts) = text_block(m, template)?;
        if cnt as usize != PANE_TEXT_ORDER.len() {
            return Some(false);
        }
        let mut pane = vec![0; PANE_SIZE as usize];
        if !m.rd(template, &mut pane) {
            return None;
        }
        let mut tbuf = vec![0; (cnt * TEXT_SIZE) as usize];
        if !m.rd(texts, &mut tbuf) {
            return None;
        }
        for (k, l) in ADDED.iter().enumerate() {
            let new_texts = *alloc;
            *alloc += cnt * TEXT_SIZE;
            let mut t = tbuf.clone();
            for i in 0..cnt as usize {
                let at = i * TEXT_SIZE as usize + 0x24;
                let sid = u32::from_le_bytes(t[at..at + 4].try_into().ok()?);
                let new = if i == 0 {
                    help_id(*l)
                } else {
                    match sid_action.get(&sid) {
                        Some(a) => *label.get(&relabel(*l, *a)).unwrap_or(&sid),
                        None => sid,
                    }
                };
                t[at..at + 4].copy_from_slice(&new.to_le_bytes());
            }
            if !m.wr(new_texts, &t) {
                return None;
            }
            let mut p = pane.clone();
            p[0x20..0x24].copy_from_slice(&new_texts.to_le_bytes());
            let index = VANILLA_PANES + group * ADDED.len() as u32 + k as u32;
            if !m.wr(panes + index * PANE_SIZE, &p) {
                return None;
            }
        }
    }
    // Publish last: a reader sees the old array or the complete new one.
    (m.set_u32(wgit + 0x24, panes) && m.set_u32(wgit + 0x20, total)).then_some(true)
}

// ---- hooks ----

/// 0 or the address of the layout byte the next empty value text stands for.
static PENDING_VALUE: AtomicU32 = AtomicU32::new(0);
static ALLOC: AtomicU32 = AtomicU32::new(0);
static STRING_ADDRS: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();

fn original(rt: &runtime::Runtime, addr: u32) -> Option<Arc<OriginalHandler>> {
    let o = rt.resolve_original(addr);
    if o.is_none() {
        eprintln!("layouts: no generated entry at {addr:#x}; hook skipped");
    }
    o.map(Arc::new)
}

/// Run the original at `addr` through `shadow`, then `then` with the caller's
/// return address. The wrapper pops the return address first so the original
/// sees its own arguments.
fn call_then(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    shadow: u32,
    then: impl FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<(), GuestFault> + Send + 'static,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(4);
    ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
        then(cpu, ctx)?;
        Ok(TransferAbi::ret(ret_pc))
    })
}

fn push(mem: &mut Guest, cpu: &mut CpuAbi, v: u32) -> Result<(), GuestFault> {
    cpu.esp = cpu.esp.wrapping_sub(4);
    mem.set_u32(cpu.esp, v)
}

/// Install every hook. A missing original (an image with other entries) skips
/// that hook and says so.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<()> {
    if std::env::var_os("H2_NO_LAYOUTS").is_some() {
        return Ok(());
    }
    {
        let mut mem = rt.memory();
        mem.map_zeroed(DATA_BASE, DATA_SIZE, Permissions::RW)?;
        mem.map_zeroed(SHADOW_BASE, 0x1000, Permissions::RX)?;
        let addrs = write_strings(&mut *mem, DATA_BASE).ok_or_else(|| anyhow::anyhow!("layouts: strings"))?;
        let used = addrs.last().map_or(0, |(id, a)| a + strings().iter().find(|(i, _)| i == id).map_or(0, |(_, t)| t.len() as u32 + 1));
        ALLOC.store((used + 0xff) & !0xff, Ordering::SeqCst);
        let _ = STRING_ADDRS.set(addrs);
    }
    let mut shadow = SHADOW_BASE;
    let mut next_shadow = || {
        shadow += 4;
        shadow - 4
    };
    rt.edit_dispatch(|d| {
        // Layout table: original, then ours for bytes 4 to 6.
        if let Some(o) = original(rt, ACTION_TABLE_BUILD) {
            let sh = alias(d, &mut next_shadow, &o, ACTION_TABLE_BUILD);
            d.register_override(ACTION_TABLE_BUILD, "layouts:table", move |cpu, ctx, _| {
                let player = cpu.eax;
                call_then(cpu, ctx, sh, move |_, ctx| {
                    let mut mem = ctx.runtime().memory();
                    apply_action_table(&mut *mem, player);
                    Ok(())
                })
            });
        }
        // List size.
        if let Some(o) = original(rt, LIST_DATA_NEW) {
            d.register_override(LIST_DATA_NEW, "layouts:list", move |cpu, ctx, _| {
                {
                    let mut mem = ctx.runtime().memory();
                    let (name, count) = (mem.u32(cpu.esp + 4)?, mem.u32(cpu.esp + 8)?);
                    let n = list_count(name, count);
                    if n != count {
                        mem.set_u32(cpu.esp + 8, n)?;
                    }
                }
                Ok(o.invoke(cpu, ctx, LIST_DATA_NEW))
            });
        }
        // Row text.
        if let Some(o) = original(rt, ROW_TEXT) {
            d.register_override(ROW_TEXT, "layouts:row-text", move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let widget = mem.u32(cpu.esp + 4)?;
                let row = mem.u16(widget + 0x70).map_or(0, |v| v as i16);
                let Some(l) = layout_of_row(row) else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, ROW_TEXT));
                };
                row_text(cpu, ctx, &mut mem, widget, name_id(l))
            });
        }
        // Row chosen.
        if let Some(o) = original(rt, ROW_SELECTED) {
            let sh = alias(d, &mut next_shadow, &o, ROW_SELECTED);
            d.register_override(ROW_SELECTED, "layouts:row-selected", move |cpu, ctx, _| {
                let row = {
                    let mem = ctx.runtime().memory();
                    let p = mem.u32(cpu.esp + 4)?;
                    mem.u16(p).map_or(0, |v| v as i16)
                };
                call_then(cpu, ctx, sh, move |_, ctx| {
                    if let Some(l) = layout_of_row(row) {
                        ctx.runtime().memory().set_u8(EDIT_LAYOUT, l.preset())?;
                    }
                    Ok(())
                })
            });
        }
        // Initial row.
        if let Some(o) = original(rt, INITIAL_ROW) {
            d.register_override(INITIAL_ROW, "layouts:initial-row", move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let byte = mem.u8(EDIT_LAYOUT)?;
                let Some(l) = Layout::from_preset(byte).filter(|l| !l.is_retail()) else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, INITIAL_ROW));
                };
                let this = cpu.ecx;
                cpu.eax = this + 0x614;
                push(&mut mem, cpu, u32::from(l.preset()))?;
                ctx.call_guest(cpu, &mut mem, LIST_SELECT, move |cpu, _| {
                    cpu.ecx = this;
                    Ok(TransferAbi::jump(SCREEN_REFRESH))
                })
            });
        }
        // Pane for a row.
        if let Some(o) = original(rt, PANE_SET) {
            d.register_override(PANE_SET, "layouts:pane", move |cpu, ctx, _| {
                {
                    let mut mem = ctx.runtime().memory();
                    if mem.u16(cpu.ecx + 0x70)? as i16 == SCREEN_ID {
                        let p = mem.u32(cpu.esp + 4)?;
                        let row = mem.u16(p)? as i16;
                        let base = mem.u8(cpu.ecx + 0x5f3)? as i8;
                        if let Some(r) = pane_row(row, base) {
                            mem.set_u16(p, r as u16)?;
                        }
                    }
                }
                Ok(o.invoke(cpu, ctx, PANE_SET))
            });
        }
        // The screen's tag: add panes once.
        if let Some(o) = original(rt, SCREEN_TAG) {
            let sh = alias(d, &mut next_shadow, &o, SCREEN_TAG);
            let o2 = Arc::clone(&o);
            d.register_override(SCREEN_TAG, "layouts:panes", move |cpu, ctx, _| {
                let is_ours = {
                    let mem = ctx.runtime().memory();
                    mem.u16(cpu.eax + 0x70).is_ok_and(|v| v as i16 == SCREEN_ID)
                };
                if !is_ours {
                    return Ok(o2.invoke(cpu, ctx, SCREEN_TAG));
                }
                call_then(cpu, ctx, sh, |cpu, ctx| {
                    if cpu.eax != 0 {
                        let mut mem = ctx.runtime().memory();
                        let mut alloc = ALLOC.load(Ordering::SeqCst);
                        match extend_panes(&mut *mem, cpu.eax, &mut alloc) {
                            Some(true) => ALLOC.store(alloc, Ordering::SeqCst),
                            Some(false) => {}
                            None => eprintln!("layouts: could not extend the layout panes"),
                        }
                    }
                    Ok(())
                })
            });
        }
        // Value text on the Controls list and the Edit Profile screen: the
        // game gives a layout byte it does not know the empty id 0.
        if let Some(o) = original(rt, TEXT_SET) {
            d.register_override(TEXT_SET, "layouts:text", move |cpu, ctx, _| {
                let pending = PENDING_VALUE.load(Ordering::Relaxed);
                if pending != 0 {
                    let mut mem = ctx.runtime().memory();
                    if mem.u32(cpu.esp + 4)? == 0 {
                        PENDING_VALUE.store(0, Ordering::Relaxed);
                        if let Some(l) = mem.u8(pending).ok().and_then(Layout::from_preset).filter(|l| !l.is_retail()) {
                            mem.set_u32(cpu.esp + 4, name_id(l))?;
                        }
                    }
                }
                Ok(o.invoke(cpu, ctx, TEXT_SET))
            });
        }
        for (addr, name) in [(PROFILE_ROWS, "layouts:profile-rows"), (CONTROLS_ROW_TEXT, "layouts:controls-row")] {
            if let Some(o) = original(rt, addr) {
                let sh = alias(d, &mut next_shadow, &o, addr);
                d.register_override(addr, name, move |cpu, ctx, _| {
                    let at = if addr == PROFILE_ROWS { cpu.ecx + PROFILE_COPY_LAYOUT } else { EDIT_LAYOUT };
                    PENDING_VALUE.store(at, Ordering::Relaxed);
                    call_then(cpu, ctx, sh, |_, _| {
                        PENDING_VALUE.store(0, Ordering::Relaxed);
                        Ok(())
                    })
                });
            }
        }
        // Our strings.
        if let Some(o) = original(rt, LOCALE_FIND) {
            d.register_override(LOCALE_FIND, "layouts:strings", move |cpu, ctx, _| {
                if is_ours(cpu.edi)
                    && let Some((_, a)) = STRING_ADDRS.get().and_then(|v| v.iter().find(|(i, _)| *i == cpu.edi))
                {
                    cpu.eax = *a;
                    return ctx.runtime().memory().ret(cpu, 0);
                }
                Ok(o.invoke(cpu, ctx, LOCALE_FIND))
            });
        }
    });
    Ok(())
}

/// Register an override at a shadow address that runs the original at `addr`.
fn alias(d: &mut Dispatch, next: &mut impl FnMut() -> u32, o: &Arc<OriginalHandler>, addr: u32) -> u32 {
    let sh = next();
    let o = Arc::clone(o);
    d.register_override(sh, "layouts:original", move |cpu, ctx, _| Ok(o.invoke(cpu, ctx, addr)));
    sh
}

/// Set a row's text to `id` by the same two calls the game makes: find the
/// item's text widget (this = item, arguments 0, 0, 6), then set its text.
/// Registers the original preserves (ESI, EDI) are restored at the end.
fn row_text(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
    widget: u32,
    id: u32,
) -> Result<TransferAbi, GuestFault> {
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(12); // return address and the two arguments
    let (esi, edi) = (cpu.esi, cpu.edi);
    for v in [6, 0, 0] {
        push(mem, cpu, v)?;
    }
    cpu.ecx = widget;
    ctx.call_guest(cpu, mem, TEXT_WIDGET_GET, move |cpu, ctx| {
        let text = cpu.eax;
        let finish = move |cpu: &mut CpuAbi| {
            cpu.esi = esi;
            cpu.edi = edi;
            Ok(TransferAbi::ret(ret_pc))
        };
        if text == 0 {
            return finish(cpu);
        }
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        cpu.edi = text;
        cpu.esi = widget;
        push(&mut mem, cpu, id)?;
        ctx.call_guest(cpu, &mut mem, TEXT_SET, move |cpu, _| finish(cpu))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A sparse byte map standing in for guest memory.
    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    fn put(f: &mut Fake, addr: u32, bytes: &[u8]) {
        assert!(f.wr(addr, bytes));
    }

    #[test]
    fn rows_four_to_six_are_ours_and_zero_to_three_are_not() {
        for r in 0..4 {
            assert_eq!(layout_of_row(r), None);
        }
        assert_eq!(layout_of_row(4), Some(Layout::BumperJumper));
        assert_eq!(layout_of_row(5), Some(Layout::Recon));
        assert_eq!(layout_of_row(6), Some(Layout::WalkieTalkie));
        assert_eq!(layout_of_row(7), None);
        assert_eq!(layout_of_row(-1), None);
    }

    #[test]
    fn only_the_layout_list_grows() {
        assert_eq!(list_count(LIST_NAME, 4), 7);
        assert_eq!(list_count(LIST_NAME, 2), 2);
        assert_eq!(list_count(0x1234, 4), 4);
    }

    #[test]
    fn the_table_is_rewritten_for_added_layouts_only() {
        for byte in 0u8..=255 {
            let mut f = Fake::default();
            let settings = SETTINGS_BASE + SETTINGS_STRIDE * 2 + 4;
            let table = TABLE_BASE + TABLE_STRIDE * 2 + TABLE_SLOTS;
            put(&mut f, settings, &[byte]);
            put(&mut f, table, &[0xaa; 16]);
            let wrote = apply_action_table(&mut f, 2);
            let mut got = [0; 16];
            assert!(f.rd(table, &mut got));
            match Layout::from_preset(byte).filter(|l| !l.is_retail()) {
                Some(l) => {
                    assert!(wrote, "{byte}");
                    assert_eq!(got, l.action_map());
                }
                None => {
                    // Retail layouts and unknown bytes: exactly what the
                    // original left there.
                    assert!(!wrote, "{byte}");
                    assert_eq!(got, [0xaa; 16], "{byte}");
                }
            }
        }
    }

    #[test]
    fn bumper_jumper_puts_jump_on_white_in_slot_zero() {
        let mut f = Fake::default();
        put(&mut f, SETTINGS_BASE + 4, &[4]);
        put(&mut f, TABLE_BASE + TABLE_SLOTS, &[0; 16]);
        assert!(apply_action_table(&mut f, 0));
        assert_eq!(f.u8(TABLE_BASE + TABLE_SLOTS), Some(Button::White.index()));
    }

    #[test]
    fn pane_rows_leave_retail_alone_and_map_ours_past_the_retail_panes() {
        for base in [0i8, 4] {
            for row in 0..4 {
                assert_eq!(pane_row(row, base), None);
            }
        }
        // Standard diagram: panes 8, 9, 10. Legacy diagram: 11, 12, 13.
        for (k, row) in [4i16, 5, 6].into_iter().enumerate() {
            assert_eq!(i16::from(0i8) + pane_row(row, 0).unwrap(), 8 + k as i16);
            assert_eq!(i16::from(4i8) + pane_row(row, 4).unwrap(), 11 + k as i16);
        }
    }

    #[test]
    fn our_ids_are_recognised_and_distinct() {
        let all = strings();
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert!(!is_ours(0x0700_0001));
        assert!(!is_ours(0));
        assert!(!is_ours(0xffff_ffff));
    }

    #[test]
    fn strings_are_written_nul_terminated_and_addressable() {
        let mut f = Fake::default();
        let addrs = write_strings(&mut f, 0x1000).unwrap();
        assert_eq!(addrs.len(), strings().len());
        for (id, a) in addrs {
            let text = strings().into_iter().find(|(i, _)| *i == id).unwrap().1;
            let mut b = vec![0; text.len() + 1];
            assert!(f.rd(a, &mut b));
            assert_eq!(&b[..text.len()], text.as_bytes());
            assert_eq!(b[text.len()], 0);
        }
    }

    #[test]
    fn relabeling_moves_actions_to_their_new_buttons() {
        // Bumper Jumper: the slot at A (jump by default) shows swap grenades,
        // the White slot (flashlight) shows jump, the Black slot shows melee.
        use Action::*;
        assert_eq!(relabel(Layout::BumperJumper, Jump), SwitchGrenades);
        assert_eq!(relabel(Layout::BumperJumper, Flashlight), Jump);
        assert_eq!(relabel(Layout::BumperJumper, SwitchGrenades), Melee);
        assert_eq!(relabel(Layout::BumperJumper, Melee), ActionReload);
        assert_eq!(relabel(Layout::BumperJumper, ActionReload), Flashlight);
        // Untouched slots keep their label, and Default relabels to itself.
        for l in ADDED {
            assert_eq!(relabel(l, Fire), Fire);
            assert_eq!(relabel(l, Crouch), Crouch);
        }
        for a in Action::GAMEPLAY {
            assert_eq!(relabel(Layout::Default, a), a);
        }
        // A relabel is a permutation of the slots.
        for l in ADDED {
            let mut seen = std::collections::HashSet::new();
            for a in Action::GAMEPLAY {
                assert!(seen.insert(relabel(l, a)), "{l:?}");
            }
        }
    }

    /// A tag with 8 panes of 13 texts, text sids `base + pane*100 + i`.
    fn fake_tag(f: &mut Fake) -> u32 {
        let (wgit, panes, texts) = (0x10_0000u32, 0x11_0000u32, 0x12_0000u32);
        put(f, wgit + 4, &(SCREEN_ID as u16).to_le_bytes());
        put(f, wgit + 0x20, &VANILLA_PANES.to_le_bytes());
        put(f, wgit + 0x24, &panes.to_le_bytes());
        for p in 0..VANILLA_PANES {
            let pane = panes + p * PANE_SIZE;
            let t = texts + p * 13 * TEXT_SIZE;
            put(f, pane, &[p as u8; PANE_SIZE as usize]);
            put(f, pane + 0x1c, &13u32.to_le_bytes());
            put(f, pane + 0x20, &t.to_le_bytes());
            for i in 0..13 {
                // Every pane has the same label per slot, like the retail
                // default panes; the help text differs.
                let sid = if i == 0 { 0x900 + p } else { 0x500 + i };
                put(f, t + i * TEXT_SIZE, &[0xee; TEXT_SIZE as usize]);
                put(f, t + i * TEXT_SIZE + 0x24, &sid.to_le_bytes());
            }
        }
        wgit
    }

    #[test]
    fn panes_are_extended_once_and_retail_panes_are_untouched() {
        let mut f = Fake::default();
        let wgit = fake_tag(&mut f);
        let old = f.u32(wgit + 0x24).unwrap();
        let mut before = vec![0; (8 * PANE_SIZE) as usize];
        assert!(f.rd(old, &mut before));
        let mut alloc = 0x20_0000;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(true));
        assert_eq!(f.u32(wgit + 0x20), Some(14));
        let new = f.u32(wgit + 0x24).unwrap();
        // The first eight panes are byte for byte the originals.
        let mut after = vec![0; (8 * PANE_SIZE) as usize];
        assert!(f.rd(new, &mut after));
        assert_eq!(before, after);
        // Bumper Jumper (pane 8): help text ours, jump's slot (slot 9, sid
        // 0x509) shows swap grenades (slot 10's sid 0x50a) and so on.
        let texts = f.u32(new + 8 * PANE_SIZE + 0x20).unwrap();
        let sid = |i: u32| f.u32(texts + i * TEXT_SIZE + 0x24).unwrap();
        assert_eq!(sid(0), help_id(Layout::BumperJumper));
        assert_eq!(sid(9), 0x50a); // A slot: swap grenades
        assert_eq!(sid(11), 0x509); // White slot: jump
        assert_eq!(sid(10), 0x508); // Black slot: melee
        assert_eq!(sid(8), 0x506); // B slot: reload
        assert_eq!(sid(6), 0x50b); // X slot: flashlight
        assert_eq!(sid(5), 0x505); // fire stays
        // Legacy-diagram clones come after the standard ones.
        let legacy = f.u32(new + 11 * PANE_SIZE + 0x20).unwrap();
        assert_eq!(f.u32(legacy + 0x24), Some(help_id(Layout::BumperJumper)));
        // A second call changes nothing.
        let end = alloc;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        assert_eq!(alloc, end);
    }

    #[test]
    fn other_screens_and_odd_tags_are_left_alone() {
        let mut f = Fake::default();
        let wgit = fake_tag(&mut f);
        put(&mut f, wgit + 4, &39u16.to_le_bytes());
        let mut alloc = 0x20_0000;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        put(&mut f, wgit + 4, &(SCREEN_ID as u16).to_le_bytes());
        put(&mut f, wgit + 0x20, &5u32.to_le_bytes());
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        assert_eq!(alloc, 0x20_0000);
    }
}
