//! Answer the map cache status `0x213f10` from its last result while
//! nothing it reads has changed ([`engine::map_status`], docs/runtime/cache-slots.md, "The status poll").
//!
//! A `before` hook looks the call up: on a hit it sets EAX, ECX, EDX and
//! the arithmetic flags as the original left them for the same key and
//! returns; otherwise the original runs and an `after` hook keeps its
//! answer. Registered on light hosts, with the other sim skips.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::map_status::{Answer, Cache, Lookup};
use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::CpuAbi;

pub struct StatusPoll;

impl Feature for StatusPoll {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sim-status-poll",
            "Answer the map cache status from its last result",
        )
        .doc("docs/runtime/cache-slots.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The cache is a memo of a pure function of guest memory, checked
        // against that memory on every call: a loaded snapshot needs nothing.
        b.snapshot(SnapshotPolicy::Stateless);
        let cache = Arc::new(Mutex::new(Cache::new()));
        let noted = AtomicBool::new(false);
        let before = Arc::clone(&cache);
        b.raw()
            .before(
                &points::MAP_STATUS,
                "answer from the last result",
                move |c| {
                    let lookup = {
                        let mem = c.ctx.memory();
                        lock(&before).lookup(&*mem, c.cpu.esp, c.cpu.eax)
                    };
                    match lookup {
                        Lookup::Hit(answer) => {
                            apply(c.cpu, &answer);
                            Ok(Flow::Return(c.ret(0)?))
                        }
                        Lookup::Miss => Ok(Flow::Continue),
                        Lookup::Bypass(why) => {
                            if !noted.swap(true, Ordering::Relaxed) {
                                eprintln!(
                                    "sim-status-poll: the original answers ({why}); later refusals are counted, not logged"
                                );
                            }
                            Ok(Flow::Continue)
                        }
                    }
                },
            )
            .optional();
        b.raw()
            .after(
                &points::MAP_STATUS,
                "keep the original's answer",
                move |c| {
                    let answer = answer_of(c.cpu);
                    let mem = c.ctx.memory();
                    // The original returned: ESP is the entry ESP plus the
                    // return address it popped.
                    lock(&cache).record(&*mem, c.cpu.esp.wrapping_sub(4), answer);
                    Ok(())
                },
            )
            .optional();
        Ok(())
    }
}

fn lock(cache: &Mutex<Cache>) -> MutexGuard<'_, Cache> {
    cache.lock().unwrap_or_else(|e| e.into_inner())
}

fn answer_of(cpu: &CpuAbi) -> Answer {
    Answer::of(cpu.eax, cpu.ecx, cpu.edx, cpu.eflags, cpu.eflags_defined)
}

fn apply(cpu: &mut CpuAbi, answer: &Answer) {
    cpu.eax = answer.eax;
    cpu.ecx = answer.ecx;
    cpu.edx = answer.edx;
    (cpu.eflags, cpu.eflags_defined) = answer.merge_flags(cpu.eflags, cpu.eflags_defined);
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::map_status::{LOCALE, SLOT_PATHS};
    use hooks::{Base, Plan, Registry, Switches};
    use runtime::exec::HARNESS_EXIT;
    use runtime::guest::Permissions;
    use runtime::{Dispatch, Event, ExecCtx, Guest, Runner, Runtime};
    use std::sync::atomic::AtomicU32;

    const STACK: u32 = 0x0060_0e00;
    const PATH: u32 = 0x0046_0000;
    const MAIN_MENU: &[u8] = b"d:\\maps\\mainmenu.map\0";

    fn plan(switches: &str) -> Plan {
        let mut registry = Registry::new(Switches::parse(switches).unwrap());
        registry.add(&StatusPoll).unwrap();
        registry.build(&Base).unwrap()
    }

    #[test]
    fn the_feature_places_a_before_and_an_after_hook() {
        let on = plan("");
        let chain = &on.chains[&points::MAP_STATUS.base];
        assert_eq!((chain.before.len(), chain.after.len()), (1, 1));
        assert!(chain.implement.is_none());
    }

    /// A stand-in original: 3 when slot 0 holds the path, else 0, with
    /// ECX, EDX and the flags derived from ESP and the answer.
    fn runtime(calls: &Arc<AtomicU32>) -> Arc<Runtime> {
        let mut d = Dispatch::default();
        let calls = Arc::clone(calls);
        d.register_generated_rust(points::MAP_STATUS.base, move |cpu, ctx, _| {
            calls.fetch_add(1, Ordering::Relaxed);
            let mut mem = ctx.memory();
            let mut path = [0u8; 0x20];
            mem.read_bytes(cpu.eax, &mut path, 1)?;
            let mut slot = [0u8; 0x20];
            mem.read_bytes(SLOT_PATHS, &mut slot, 1)?;
            let status = if path == slot { 3 } else { 0 };
            // Scribble on the dead stack as the original does.
            mem.write_bytes(cpu.esp - 0x100, &[0xee; 0x40], 1)?;
            cpu.eax = status;
            cpu.ecx = cpu.esp ^ 0x55;
            cpu.edx = status * 7;
            cpu.eflags = (cpu.eflags & !0x8d5) | if status == 3 { 0x4 } else { 0x40 };
            cpu.eflags_defined |= 0x8d5;
            mem.ret(cpu, 0)
        })
        .unwrap();
        let mut g = Guest::new();
        // The slot paths, the copy requests and the queue block (zeroed,
        // so its paths end inside it).
        g.map_zeroed(0x0055_7000, 0x6000, Permissions::RW).unwrap();
        g.map_zeroed(LOCALE & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        g.map_zeroed(PATH, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(STACK & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        g.map_zeroed(points::MAP_STATUS.base & !0xfff, 0x1000, Permissions::RX)
            .unwrap();
        g.write_bytes(PATH, MAIN_MENU, 1).unwrap();
        let rt = Runtime::new(g, d);
        plan("").install(&rt).unwrap();
        rt
    }

    fn call(rt: &Arc<Runtime>, eflags: u32) -> CpuAbi {
        rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
        let mut cpu = CpuAbi {
            esp: STACK,
            eax: PATH,
            ebx: 0x1111,
            esi: 0x2222,
            edi: 0x3333,
            ebp: 0x4444,
            eflags,
            ..Default::default()
        };
        let mut ctx = ExecCtx::new(Arc::clone(rt), 1);
        let t = Runner::default().run(&mut cpu, &mut ctx, points::MAP_STATUS.base);
        assert!(matches!(ctx.event(t.event_id), Some(Event::HarnessExit)));
        assert_eq!(cpu.esp, STACK + 4);
        assert_eq!(
            (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp),
            (0x1111, 0x2222, 0x3333, 0x4444)
        );
        cpu
    }

    #[test]
    fn a_repeat_is_answered_without_the_original_until_a_slot_changes() {
        let calls = Arc::new(AtomicU32::new(0));
        let rt = runtime(&calls);
        let first = call(&rt, 0x202);
        assert_eq!((first.eax, calls.load(Ordering::Relaxed)), (0, 1));
        let again = call(&rt, 0x202);
        assert_eq!(calls.load(Ordering::Relaxed), 1, "answered from the cache");
        assert_eq!(
            (
                again.eax,
                again.ecx,
                again.edx,
                again.eflags,
                again.eflags_defined
            ),
            (
                first.eax,
                first.ecx,
                first.edx,
                first.eflags,
                first.eflags_defined
            )
        );
        // Only the status flags are the answer's: DF stays the caller's.
        let df = call(&rt, 0x602);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(df.eflags, first.eflags | 0x400);

        rt.memory().write_bytes(SLOT_PATHS, MAIN_MENU, 1).unwrap();
        let resident = call(&rt, 0x202);
        assert_eq!((resident.eax, resident.edx), (3, 21));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(call(&rt, 0x202).eax, 3);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn a_locale_other_than_c_always_runs_the_original() {
        let calls = Arc::new(AtomicU32::new(0));
        let rt = runtime(&calls);
        rt.memory().set_u32(LOCALE, 1).unwrap();
        for n in 1..=3 {
            call(&rt, 0x202);
            assert_eq!(calls.load(Ordering::Relaxed), n);
        }
    }
}
