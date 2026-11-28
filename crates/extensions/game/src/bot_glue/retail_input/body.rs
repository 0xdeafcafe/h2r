//! The action builder's body in Rust, in the phantom executor only
//! (docs/perf/sim-phantom-build.md, "The builder body in Rust"), always
//! on.
//!
//! Every seat of `engine::sim_seats::phantom_body` goes into the
//! executor's dispatch at its entry and its return sites. A seat runs a
//! segment; a call it ends with is made in place (`ExecCtx::call_inline`)
//! and the seat carries on at the call's site, so a build hands the
//! runner no call while every callee returns plainly. A callee that is a
//! seat too (the control update under the builder, the target search
//! under it) enters its own Rust through the entry table. A seat that
//! declines runs the original from the same point: at its entry, or at
//! the site's generated resume. Nothing in the live game's dispatch
//! changes, so the `aim-assist` feature's hooks at `0x185be0` and
//! `0x1a50a0`, which `prefer_generated` already turns off here, stay off.
use engine::sim_seats::{self, Seat};
use runtime::{CpuAbi, Dispatch, ExecCtx, Fallback, GuestFault, Inline, TransferAbi, abi};
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Seat segments that declined and ran the original.
pub(super) static DECLINED: AtomicU64 = AtomicU64::new(0);
/// Calls made in place that came back to their site.
pub(super) static RETURNED: AtomicU64 = AtomicU64::new(0);
/// Calls left to the runner, by `Fallback::index`.
pub(super) static HANDED: [AtomicU64; Fallback::ALL.len()] =
    [const { AtomicU64::new(0) }; Fallback::ALL.len()];

/// The (seat, point) declines already said.
static SAID: Mutex<BTreeSet<(&'static str, u32)>> = Mutex::new(BTreeSet::new());

/// Install every phantom seat in `d`, after `prefer_generated`.
pub(super) fn install(d: &mut Dispatch) {
    for seat in sim_seats::phantom_body() {
        let run = move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32| drive(seat, cpu, ctx, pc);
        let _ = d.register_override(seat.point.base, format!("phantom body: {}", seat.name), run);
        d.implement_resumes(seat.point.base, seat.sites, run);
    }
}

/// Run `seat` from `pc` (its entry, or one of its sites) until a transfer
/// leaves for the runner.
fn drive(seat: &'static Seat, cpu: &mut CpuAbi, ctx: &mut ExecCtx, mut pc: u32) -> Result<TransferAbi, GuestFault> {
    loop {
        let site = pc != seat.point.base;
        if site && seat.sites.binary_search(&pc).is_err() {
            return original(cpu, ctx, pc, true);
        }
        let step = {
            let mut mem = ctx.memory();
            (seat.run)(cpu, &mut mem, pc)?
        };
        let Some(transfer) = step else {
            declined(seat, pc);
            return original(cpu, ctx, pc, site);
        };
        let at = transfer.resume_pc;
        if transfer.kind != abi::CALL || seat.sites.binary_search(&at).is_err() {
            return Ok(transfer);
        }
        match ctx.call_inline(cpu, transfer.target_pc, at) {
            Inline::Returned => {
                RETURNED.fetch_add(1, Ordering::Relaxed);
                pc = at;
            }
            Inline::Leave { transfer, why } => {
                HANDED[why.index()].fetch_add(1, Ordering::Relaxed);
                return Ok(transfer);
            }
        }
    }
}

/// A decline, counted and said once a seat and point.
fn declined(seat: &'static Seat, pc: u32) {
    DECLINED.fetch_add(1, Ordering::Relaxed);
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said.insert((seat.name, pc)) {
        eprintln!(
            "phantoms: the action body {} declined at {pc:#x}; that call runs the guest from there",
            seat.name
        );
    }
}

/// The original at `pc`: its entry, or the generated resume at a site.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32, site: bool) -> Result<TransferAbi, GuestFault> {
    let found = if site {
        ctx.runtime().resolve_original_resume(pc)
    } else {
        ctx.runtime().resolve_original(pc).map(|h| (h, pc))
    };
    let (handler, selector) = found.ok_or(GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    })?;
    Ok(handler.invoke(cpu, ctx, selector))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phantom_body_holds_the_builder_its_search_and_the_seats_under_it() {
        let points: BTreeSet<u32> = sim_seats::phantom_body().map(|s| s.point.base).collect();
        for at in [0x186ab0, 0x151110, 0x185be0, 0x185b30, 0x1a4900, 0x1a5370, 0x1a66f0] {
            assert!(points.contains(&at), "{at:#x}");
        }
        // The aim-assist cone keeps per-player host state: never a phantom seat.
        assert!(!points.contains(&0x1a50a0));
        for seat in sim_seats::phantom_body() {
            assert!(seat.sites.windows(2).all(|w| w[0] < w[1]), "{}: sites unsorted", seat.name);
        }
    }
}
