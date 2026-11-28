//! The channel manager and graph lookups. Guest frames carry all live
//! state across calls, including calls that leave the animation subsystem.
use crate::sim_native::{
    Memory,
    plan::{Plan, segment},
};
use guest_layout::points;
use crate::sim_native;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi, abi};
use std::sync::LazyLock;

type Bus<'a> = Plan<'a, Guest, 128>;
type Handler = fn(&mut CpuAbi, &mut Bus<'_>, u32) -> sim_native::Result;

/// One retail entry and the original selectors after its calls.
pub struct Body {
    pub point: points::Point,
    pub entry: u32,
    pub end: u32,
    pub sites: &'static [u32],
    handler: Handler,
}

macro_rules! body {
    ($entry:expr, $end:expr, $sites:expr, $handler:expr $(,)?) => {
        Body {
            point: points::Point::new(
                concat!("ANIM_MANAGER_", stringify!($entry)),
                $entry,
                points::Sig::Opaque,
                "docs/engine/animation-manager.md",
            ),
            entry: $entry,
            end: $end,
            sites: $sites,
            handler: $handler,
        }
    };
}

pub mod arm;

mod actions;
mod advance;
mod channel_blend;
mod channel_eval;
mod channel_select;
mod final_channels;
mod graph;
mod low;
mod queries;
mod tracks;
mod upper;

static BODIES: LazyLock<Vec<&'static Body>> = LazyLock::new(|| {
    let mut rows: Vec<_> = [
        low::BODIES,
        upper::BODIES,
        tracks::BODIES,
        queries::BODIES,
        actions::BODIES,
        channel_eval::BODIES,
        channel_select::BODIES,
        channel_blend::BODIES,
        advance::BODIES,
        final_channels::BODIES,
        graph::BODIES,
    ]
    .into_iter()
    .flatten()
    .collect();
    rows.sort_unstable_by_key(|body| body.entry);
    rows
});

pub fn bodies() -> impl Iterator<Item = &'static Body> {
    BODIES.iter().copied()
}

fn lookup(pc: u32) -> Option<&'static Body> {
    let next = BODIES.partition_point(|row| row.entry <= pc);
    let body = *BODIES.get(next.checked_sub(1)?)?;
    (pc == body.entry || (pc < body.end && body.sites.binary_search(&pc).is_ok())).then_some(body)
}

fn bad(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn push(c: &mut CpuAbi, g: &mut Bus<'_>, v: u32) -> Result<(), GuestFault> {
    crate::sim_physics::push(c, g, v)
}

fn pop(c: &mut CpuAbi, g: &Bus<'_>) -> Result<u32, GuestFault> {
    crate::sim_physics::pop(c, g)
}

fn run_body(
    body: &Body,
    c: &mut CpuAbi,
    g: &mut Guest,
    pc: u32,
) -> Result<Option<TransferAbi>, GuestFault> {
    segment::<128, _>(c, g, pc, body.entry..body.end, body.sites, |c, g, pc| {
        evaluate(body, c, g, pc)
    })
}

fn evaluate(body: &Body, c: &mut CpuAbi, g: &mut Bus<'_>, pc: u32) -> sim_native::Result {
    g.allow_large_segment();
    (body.handler)(c, g, pc)
}

/// Run exactly one entry or return segment against the live guest frame.
pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<Option<TransferAbi>, GuestFault> {
    let Some(body) = lookup(pc) else {
        return Ok(None);
    };
    run_body(body, c, g, pc)
}

/// Keep consecutive calls and returns between installed, unshared manager
/// bodies in this invocation. Each segment commits before entering the
/// next. If that next segment declines, the scheduler receives its incoming
/// transfer with the guest frame intact, exactly as at an external call.
pub fn drive(
    c: &mut CpuAbi,
    g: &mut Guest,
    mut pc: u32,
    available: impl Fn(&Body) -> bool,
) -> Result<Option<TransferAbi>, GuestFault> {
    let Some(mut body) = lookup(pc) else {
        return Ok(None);
    };
    let mut incoming = None;
    for _ in 0..1024 {
        let Some(transfer) = run_body(body, c, g, pc)? else {
            return Ok(incoming);
        };
        let next_pc = transfer.target_pc;
        let Some(next) = lookup(next_pc) else {
            return Ok(Some(transfer));
        };
        let can_enter = match transfer.kind {
            abi::CALL | abi::JUMP => transfer.route == abi::ENTRY && next_pc == next.entry,
            abi::RETURN => transfer.route == abi::RETURN_TARGET && next_pc != next.entry,
            _ => false,
        };
        if !can_enter || !available(next) {
            return Ok(Some(transfer));
        }
        incoming = Some(transfer);
        body = next;
        pc = next_pc;
    }
    Ok(incoming)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn declined_caller_return_preserves_the_committed_graph_result() {
        let (entry, graph, preload, resume) = (0x1c6440, 0x1daea0, 0x1dd9d0, 0x1c6466);
        let mut g = Guest::new();
        for pc in [entry, graph, preload] {
            let body = lookup(pc).unwrap();
            g.map_zeroed(pc, body.end - pc, Permissions::RX).unwrap();
        }
        let channel = 0x10000;
        let tags = channel + 0x100;
        let graph_data = channel + 0x200;
        let clips = channel + 0x300;
        g.map_zeroed(channel, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(points::TAG_ENTRIES.base, 4, Permissions::RW).unwrap();
        g.map_zeroed(sim_native::graph::ENABLED, 1, Permissions::READ).unwrap();
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        g.set_u32(tags + 8, graph_data).unwrap();
        g.set_u32(graph_data + 0x30, clips).unwrap();
        g.set_u32(channel + 4, 0x0001ffff).unwrap();
        // The children have writable frames below SP. Only the final RET
        // reads at SP, where the caller has no mapped return address.
        let sp = channel + 0x1000;
        let mut c = CpuAbi { ecx: channel, esp: sp, ..Default::default() };
        let transfer = drive(&mut c, &mut g, entry, |_| true).unwrap();
        assert_eq!(transfer, Some(TransferAbi::ret(resume)));
        assert_eq!((c.eax, c.esp), (clips + 108, sp));
        let committed = c;
        assert_eq!(run(&mut c, &mut g, resume).unwrap(), None);
        assert_eq!(c, committed);
    }

    #[test]
    fn child_refusal_keeps_the_parent_call_frame() {
        let (entry, child, site) = (0x1c6440, 0x1daea0, 0x1c6466);
        let mut g = Guest::new();
        for pc in [entry, child] {
            let body = lookup(pc).unwrap();
            g.map_zeroed(pc, body.end - pc, Permissions::RX).unwrap();
        }
        g.map_zeroed(0x10000, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(points::TAG_ENTRIES.base, 4, Permissions::RW)
            .unwrap();
        g.set_u32(points::TAG_ENTRIES.base, 0x10400).unwrap();
        g.set_u32(0x10408, 0x20000).unwrap(); // unmapped graph
        g.set_u32(0x10004, 0x0001ffff).unwrap();
        let sp = 0x10900;
        for n in 1..=4 {
            g.set_u32(sp - 4 * n, 0xfeedbeef).unwrap();
        }
        let mut c = CpuAbi {
            ecx: 0x10000,
            esp: sp,
            ebx: 1,
            esi: 2,
            ..Default::default()
        };
        assert_eq!(
            drive(&mut c, &mut g, entry, |_| true).unwrap(),
            Some(TransferAbi::call(child, site))
        );
        assert_eq!((c.esp, c.ebx, c.esi), (sp - 8, 1, 2));
        assert_eq!(g.u32(sp - 4).unwrap(), 0x0001ffff);
        assert_eq!(g.u32(sp - 8).unwrap(), site);
        assert_eq!(g.u32(sp - 12).unwrap(), 0xfeedbeef);
        assert_eq!(g.u32(sp - 16).unwrap(), 0xfeedbeef);
    }

    /// Map a body's text the way the XBE loader does: one mapping per page.
    fn loader_text(g: &mut Guest, entry: u32) {
        let body = lookup(entry).unwrap();
        let mut page = entry & !0xfff;
        while page < body.end {
            g.map_zeroed(page, 0x1000, Permissions::RX).unwrap();
            page += 0x1000;
        }
        assert!(g.mappings().len() >= 2, "{entry:#x}'s text crosses a page");
    }

    #[test]
    fn base_pose_entry_runs_from_text_in_two_page_mappings() {
        // Ascension: `anim ANIM_MANAGER_0x1c6f60: fault at 0x1c6f60`. Its
        // text 1c6f60..1c70f7 crosses 0x1c7000.
        let (entry, track, resume) = (0x1c6f60, 0x1daea0, 0x1c6f8c);
        let mut g = Guest::new();
        loader_text(&mut g, entry);
        let (channel, tags, sp) = (0x10000, 0x10100, 0x10800);
        g.map_zeroed(channel, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(points::TAG_ENTRIES.base, 4, Permissions::RW).unwrap();
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        g.set_u32(tags + 2 * 16 + 8, 0x5000).unwrap();
        g.set_u32(channel, 0x0001_0002).unwrap();
        g.set_u32(channel + 4, 0x77).unwrap();
        let mut c = CpuAbi { ebx: channel, esp: sp, edi: 0xed1, ..Default::default() };
        assert_eq!(run(&mut c, &mut g, entry).unwrap(), Some(TransferAbi::call(track, resume)));
        assert_eq!((c.esp, c.edx, c.edi), (sp - 20, 0x5000, u32::MAX));
        assert_eq!(g.u32(sp - 20).unwrap(), resume);
        assert_eq!(g.u32(sp - 16).unwrap(), 0x77);
        assert_eq!(g.u32(sp - 12).unwrap(), 0xed1);
    }

    #[test]
    fn find_transition_entry_runs_from_text_in_two_page_mappings() {
        // 03a_oldmombasa: `anim ANIM_MANAGER_0x1ccda0: fault at 0x1ccda0`.
        // Its text 1ccda0..1cd070 crosses 0x1cd000. With transitions off
        // (0x46fbf5 clear) it clears both outputs and returns.
        let entry = 0x1ccda0;
        let (sp, found, best) = (0x10800, 0x10900, 0x10a00);
        let setup = |with_flags: bool| {
            let mut g = Guest::new();
            loader_text(&mut g, entry);
            if with_flags {
                g.map_zeroed(0x46f000, 0x1000, Permissions::READ).unwrap();
            }
            g.map_zeroed(0x10000, 0x1000, Permissions::RW).unwrap();
            g.set_u32(sp, 0x1234).unwrap();
            g.set_u32(sp + 4, best).unwrap();
            g.set_u32(sp + 0x10, found).unwrap();
            g.set_u8(found, 0xaa).unwrap();
            g
        };
        let mut g = setup(true);
        let mut c = CpuAbi { esp: sp, esi: 0x55, ..Default::default() };
        assert_eq!(run(&mut c, &mut g, entry).unwrap(), Some(TransferAbi::ret(0x1234)));
        assert_eq!((c.esp, c.esi), (sp + 20, 0x55));
        assert_eq!((g.u8(found).unwrap(), g.u32(best).unwrap()), (0, u32::MAX));
        // A load the plan cannot make is named, and nothing is stored.
        let mut g = setup(false);
        let mut c = CpuAbi { esp: sp, ..Default::default() };
        let before = c;
        assert_eq!(run(&mut c, &mut g, entry).unwrap(), None);
        assert_eq!(c, before);
        assert_eq!(g.u8(found).unwrap(), 0xaa);
        let why = crate::sim_native::Decline::Read { addr: 0x46fbf5, len: 1 };
        assert_eq!(crate::sim_native::take_decline(), Some((entry, why)));
    }
}
