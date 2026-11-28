//! Accepted AI dialogue candidates, observed at the scheduler's entry and return.

use std::sync::{Arc, Mutex};

use ::actor_net::wire::MAX_RECORDS;
use guest_layout::{GuestMemory, points};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) unit: u32,
    pub(super) vocalization: u16,
    pub(super) tick: u32,
}

/// A tick of new lines. Budget owns the repeat transmissions per client.
#[derive(Default)]
pub(super) struct Lines(Vec<Line>);

impl Lines {
    pub(super) fn push(&mut self, line: Line) {
        if self.0.len() < MAX_RECORDS && !self.0.contains(&line) {
            self.0.push(line);
        }
    }

    pub(super) fn take(&mut self) -> Vec<Line> {
        std::mem::take(&mut self.0)
    }
}

fn candidate(m: &dyn GuestMemory, at: u32, tick: u32) -> Option<Line> {
    if at == 0 {
        return None;
    }
    // AI_DIALOGUE_ENQUEUE reads the signed vocabulary index and the full
    // salted speaker unit from the caller's winning candidate.
    let vocalization = m.u16(at).ok()?;
    let unit = m.u32(at.checked_add(4)?).ok()?;
    (vocalization <= i16::MAX as u16 && unit != u32::MAX).then_some(Line {
        unit,
        vocalization,
        tick,
    })
}

#[derive(Clone, Copy)]
struct Call {
    thread: u32,
    returned_esp: u32,
    line: Line,
}

/// The caller owns the candidate. Save it before the call and publish it
/// only if this particular invocation returns true in AL.
#[derive(Clone, Default)]
pub(super) struct Pending(Arc<Mutex<Vec<Call>>>);

impl Pending {
    pub(super) fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn enter(&self, thread: u32, returned_esp: u32, line: Option<Line>) {
        let mut calls = self.0.lock().unwrap();
        // A before-hook may answer in place of the original, or an outer
        // frame may unwind. Neither gets a return observer. A fresh entry
        // at this depth also retires abandoned frames below it.
        calls.retain(|c| c.thread != thread || c.returned_esp > returned_esp);
        if calls.len() < MAX_RECORDS
            && let Some(line) = line
        {
            calls.push(Call {
                thread,
                returned_esp,
                line,
            });
        }
    }

    fn finish(&self, thread: u32, esp: u32, eax: u32) -> Option<Line> {
        let mut calls = self.0.lock().unwrap();
        let index = calls
            .iter()
            .rposition(|c| c.thread == thread && c.returned_esp == esp)?;
        let call = calls.swap_remove(index);
        (eax as u8 != 0).then_some(call.line)
    }
}

pub(super) fn install(
    builder: &mut hooks::Builder,
    on_line: impl Fn(Line) + Send + Sync + 'static,
) -> Pending {
    let point = &points::AI_DIALOGUE_ENQUEUE;
    let points::Sig::Fn { pop, .. } = point.sig else {
        unreachable!("AI_DIALOGUE_ENQUEUE requires its callee-cleanup signature")
    };
    let pending = Pending::default();
    let entries = pending.clone();
    builder
        .raw()
        .observe_before(point, "read dialogue candidate", move |o| {
            let address = o.arg_u32("candidate");
            let m = o.guest();
            let line = super::host_tick(&*m).and_then(|tick| candidate(&*m, address?, tick));
            let returned_esp = o.cpu.esp.wrapping_add(4 + u32::from(pop));
            entries.enter(o.thread_id, returned_esp, line);
        });
    let returns = pending.clone();
    builder
        .raw()
        .observe_after(point, "capture accepted dialogue", move |o| {
            if let Some(line) = returns.finish(o.thread_id, o.cpu.esp, o.cpu.eax) {
                on_line(line);
            }
        });
    pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_line_is_queued_once() {
        use runtime::exec::HARNESS_EXIT;
        use runtime::guest::Permissions;
        use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};

        const STACK: u32 = 0x20_0800;
        const WORLD: u32 = 0x21_0000;
        const CLOCK: u32 = WORLD + 0x100;
        const CANDIDATE: u32 = WORLD + 0x200;
        const UNIT: u32 = 0x5830_0019;
        let mut guest = Guest::new();
        for address in [
            STACK,
            WORLD,
            points::SIM_GLOBALS.base,
            points::GAME_TIME.base,
        ] {
            guest
                .map_zeroed(address & !0xfff, 0x1000, Permissions::RW)
                .unwrap();
        }
        guest
            .map_zeroed(
                points::AI_DIALOGUE_ENQUEUE.base & !0xfff,
                0x1000,
                Permissions::RX,
            )
            .unwrap();
        guest.set_u32(points::SIM_GLOBALS.base, WORLD).unwrap();
        guest.set_u32(points::GAME_TIME.base, CLOCK).unwrap();
        guest.set_u32(CLOCK + 8, 83).unwrap();
        guest.set_u32(CANDIDATE + 4, UNIT).unwrap();
        let rt = Runtime::new(guest, Dispatch::default());
        let lines = Arc::new(Mutex::new(Lines::default()));
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        registry
            .add(&hooks::from_fn("dialogue-fixture", |b| {
                let output = lines.clone();
                install(b, move |line| output.lock().unwrap().push(line));
                b.raw()
                    .implement(&points::AI_DIALOGUE_ENQUEUE, "fixture", |cpu, ctx, _| {
                        cpu.eax = cpu.edi;
                        ctx.memory().ret(cpu, 28)
                    });
                Ok(())
            }))
            .unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        assert!(plan.install(&rt).unwrap().off.is_empty());

        // The candidate is argument three; the callee pops the seven arguments.
        // Offer the same accepted line twice, one rejected line at the host,
        // and one otherwise-accepted line on the client.
        for (mode, vocalization, result) in [(4, 14, 0x101), (4, 14, 1), (4, 15, 0x100), (5, 16, 1)]
        {
            {
                let mut m = rt.memory();
                m.set_u32(WORLD + 8, mode).unwrap();
                m.set_u16(CANDIDATE, vocalization).unwrap();
                m.set_u32(STACK, HARNESS_EXIT).unwrap();
                m.set_u32(STACK + 16, CANDIDATE).unwrap();
            }
            let mut cpu = CpuAbi {
                esp: STACK,
                edi: result,
                ..Default::default()
            };
            let mut ctx = ExecCtx::new(rt.clone(), 1);
            let result =
                Runner::default().run(&mut cpu, &mut ctx, points::AI_DIALOGUE_ENQUEUE.base);
            assert!(matches!(
                ctx.event(result.event_id),
                Some(Event::HarnessExit)
            ));
            assert_eq!(cpu.esp, STACK + 32);
        }
        let mut output = lines.lock().unwrap();
        assert_eq!(
            output.take(),
            vec![Line {
                unit: UNIT,
                vocalization: 14,
                tick: 83
            }]
        );
        assert!(output.take().is_empty());
    }

    #[test]
    fn nested_calls_keep_the_entry_candidate_until_their_own_return() {
        let mut m = FakeGuest::new();
        let at = 0x2000;
        let unit = 0x7300_0012;
        let tick = 52;
        m.put_u16(at, 37).put_u32(at + 4, unit);
        let line = Line {
            unit,
            vocalization: 37,
            tick,
        };
        let pending = Pending::default();

        // A nested invocation may fail while the outer one succeeds. AL
        // alone is the result, and a reused candidate buffer is harmless.
        pending.enter(1, 0x9000, candidate(&m, at, tick));
        m.put_u16(at, 16);
        pending.enter(1, 0x8000, candidate(&m, at, tick));
        assert_eq!(pending.finish(1, 0x8000, 0xab00), None);
        assert_eq!(pending.finish(2, 0x9000, 1), None);
        assert_eq!(pending.finish(1, 0x9000, 0xab01), Some(line));
        assert_eq!(pending.finish(1, 0x9000, 1), None);
    }
}
