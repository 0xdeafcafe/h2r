//! Follow the specific queued options through retail's full game start.
//! Checkpoint reactivation calls GAME_ACTIVATE directly, bypassing START_GAME.
use super::*;
use hooks::points::{Loc, Point, Sig};

static PROCESS_TRANSITION: Point = Point::new(
    "PROCESS_TRANSITION",
    0x0018_e810,
    Sig::Site,
    "docs/engine/engine-map.md",
);

static START_GAME: Point = Point::new(
    "START_GAME",
    0x0018_e8b0,
    Sig::Fn {
        args: &[("options", Loc::Stack(0))],
        ret: Some(Loc::Al),
        pop: 4,
    },
    "docs/live/live-gamestate.md",
);

pub fn reload_hook_points() -> [&'static Point; 3] {
    [&guest_layout::points::OPTIONS_QUEUE, &PROCESS_TRANSITION, &START_GAME]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Transition {
    ticket: u64,
    // Unchanged after OPTIONS_QUEUE. Another queue retires this association
    // even if it supplies byte-identical options.
    options: u32,
    process: Option<(u32, u32)>,
    start_return_esp: Option<u32>,
}

impl Transition {
    pub(super) fn queued(ticket: u64, options: u32) -> Self {
        Self {
            ticket,
            options,
            process: None,
            start_return_esp: None,
        }
    }

    fn live_at_tick(&self, thread: u32, esp: u32, pending: impl FnOnce() -> bool) -> bool {
        match self.process {
            Some((owner, frame)) => thread != owner || esp < frame,
            None => pending(),
        }
    }
}

impl Queue {
    fn begin_transition(&mut self, m: &dyn GuestMemory, thread: u32, esp: u32) {
        if let Some(t) = &mut self.transition {
            if t.process.is_none()
                && matches!(pending_options(m, t.options), Ok(PendingOptions::Reload))
            {
                t.process = Some((thread, esp));
            } else {
                self.transition = None;
            }
        }
    }

    fn entering_start(&mut self, m: &dyn GuestMemory, thread: u32, esp: u32) {
        let Some(t) = &mut self.transition else { return };
        let Some((owner, frame)) = t.process else { return };
        if owner != thread || esp > frame {
            return;
        }
        let len = OPTIONS_SIZE as usize;
        let arg = m.u32(esp.wrapping_add(4)).and_then(|ptr| m.bytes(ptr, len));
        let expected = m.bytes(t.options, len);
        let matching = matches!((arg, expected), (Ok(arg), Ok(expected)) if arg == expected);
        t.start_return_esp = matching.then(|| esp.wrapping_add(8));
    }

    fn finished_start(&mut self, thread: u32, esp: u32, succeeded: bool) -> Option<u64> {
        let owned = self.transition.take_if(|t| {
            t.process.is_some_and(|(owner, _)| owner == thread) && t.start_return_esp == Some(esp)
        });
        if !succeeded {
            return None;
        }
        if let Some(t) = owned {
            return Some(t.ticket);
        }
        self.next += 1;
        Some(self.next)
    }
}

pub(crate) fn install_reload_recovery(
    b: &mut hooks::Builder,
    queue: Shared,
    agent: crate::Agent,
    sources: Option<crate::EditorActivation>,
) {
    // A dropped continuation might never reach a transition return. Recover
    // before the next editor call, outside the ordinary agent tick chain.
    let tick = queue.clone();
    b.raw().before(&guest_layout::points::GAME_TICK, "recover an abandoned reload", move |c| {
        let (interrupted, transition) = {
            let q = lock(&tick);
            (q.repair.is_some() || q.handoff.as_ref().is_some_and(|h| h.interrupted), q.transition)
        };
        if interrupted {
            recover_before(c, &tick)?;
        }
        if let Some(t) = transition
            && !t.live_at_tick(c.ctx.thread_id, c.cpu.esp, || {
                c.ctx.runtime().memory().u8(OPTIONS_PENDING).is_ok_and(|p| p != 0)
            })
        {
            let _ = lock(&tick).transition.take_if(|current| *current == t);
        }
        Ok(hooks::Flow::Continue)
    }).first();
    let before_queue = queue.clone();
    b.raw().before(&guest_layout::points::OPTIONS_QUEUE, "reconcile interrupted queue", move |c| {
        recover_before(c, &before_queue)?;
        lock(&before_queue).transition = None;
        Ok(hooks::Flow::Continue)
    }).first();

    let process = queue.clone();
    b.raw().before(&PROCESS_TRANSITION, "reconcile editor reload", move |c| {
        recover_before(c, &process)?;
        let guest = c.ctx.runtime().memory();
        lock(&process).begin_transition(&*guest, c.ctx.thread_id, c.cpu.esp);
        Ok(hooks::Flow::Continue)
    }).first();
    let end = queue.clone();
    b.raw().observe_after(&PROCESS_TRANSITION, "finish editor transition", move |o| {
        let _ = lock(&end).transition.take_if(|t| {
            t.process.is_some_and(|(owner, _)| owner == o.thread_id)
        });
    });

    let start = queue.clone();
    b.raw().observe_before(&START_GAME, "match queued start", move |o| {
        let guest = o.guest();
        lock(&start).entering_start(&*guest, o.thread_id, o.cpu.esp);
    });
    b.raw().observe_after(&START_GAME, "publish full game start", move |o| {
        let id = lock(&queue).finished_start(o.thread_id, o.cpu.esp, o.cpu.eax & 0xff != 0);
        if let Some(id) = id {
            if let Some(publish) = &sources {
                publish(&*o.guest());
            }
            agent.game_started(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_reload_receipt_belongs_to_its_own_transition_and_full_start() {
        const OPTIONS: u32 = 0x6000;
        const FRAME: u32 = 0x9000;
        const START: u32 = FRAME - 64;
        let mut guest = FakeGuest::new();
        guest.put_u8(OPTIONS_PENDING, 1);
        guest.map(OPTIONS, OPTIONS_SIZE).map(PENDING_OPTIONS, OPTIONS_SIZE);
        guest.put_u32(START + 4, PENDING_OPTIONS);
        let mut q = Queue {
            next: 12,
            transition: Some(Transition::queued(12, OPTIONS)),
            ..Queue::default()
        };

        // An independent full start with the same map and options has its
        // own identity. Checkpoints bypass START_GAME altogether.
        q.entering_start(&guest, 1, START);
        assert_eq!(q.finished_start(1, START + 8, true), Some(13));
        q.begin_transition(&guest, 1, FRAME);
        q.entering_start(&guest, 1, START);
        assert_ne!(q.finished_start(2, START + 8, true), Some(12));
        assert_ne!(q.finished_start(1, START, true), Some(12));
        assert_eq!(q.finished_start(1, START + 8, true), Some(12));
        assert!(!q.reload_transition_pending());

        q.transition = Some(Transition::queued(11, OPTIONS));
        guest.put_u32(PENDING_OPTIONS, 17);
        q.begin_transition(&guest, 1, FRAME);
        assert!(!q.reload_transition_pending());
        assert_ne!(q.finished_start(1, START + 8, true), Some(11));
    }

    #[test]
    fn a_failed_start_retires_the_reserved_activation() {
        let mut q = Queue::default();
        let mut t = Transition::queued(4, 0x4000);
        t.process = Some((2, 0x8000));
        t.start_return_esp = Some(0x7000);
        q.next = 4;
        q.transition = Some(t);
        assert_eq!(q.finished_start(2, 0x7000, false), None);
        assert_eq!(q.finished_start(2, 0x7000, true), Some(5));
        assert!(!q.reload_transition_pending());
    }
}
