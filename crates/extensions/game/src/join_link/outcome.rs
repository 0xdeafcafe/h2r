//! A result belongs to the FindFromID request the title is driving. A
//! previously connected squad or a key registered for a QoS probe cannot
//! complete it. The host's advertised ID and its squad's XNKID may differ.
use guest_layout::memory::GuestMemory;
use hooks::{Builder, points};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct Read {
    target: [u8; 8],
    update: Option<(u32, u32)>,
    result: Option<Result<(), String>>,
}

#[derive(Default)]
pub(super) struct Outcome {
    active: AtomicBool,
    read: Mutex<Read>,
}

impl Outcome {
    pub(super) fn begin(&self, target: [u8; 8]) {
        *self.read.lock().unwrap_or_else(|e| e.into_inner()) = Read {
            target,
            ..Default::default()
        };
        self.active.store(true, Ordering::Release);
    }

    pub(super) fn clear(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub(super) fn take(&self) -> Option<Result<(), String>> {
        self.read
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result
            .take()
    }

    fn before(&self, m: &dyn GuestMemory, context: u32, thread: u32) {
        let Some(id) = context.checked_add(0x3a) else {
            return;
        };
        let mut requested = [0; 8];
        // The title hands context+0x3a to FindFromID.
        if m.read(id, &mut requested).is_ok() {
            let mut r = self.read.lock().unwrap_or_else(|e| e.into_inner());
            if r.target == requested && r.result.is_none() {
                r.update = Some((thread, context));
            }
        }
    }

    fn after(&self, m: &dyn GuestMemory, thread: u32) {
        let mut r = self.read.lock().unwrap_or_else(|e| e.into_inner());
        let Some((owner, context)) = r.update else {
            return;
        };
        if owner != thread {
            return;
        }
        r.update = None;
        let Some(at) = context.checked_add(0x104) else {
            return;
        };
        r.result = match m.u32(at) {
            // 0x6fdba sets 2 after promoting the target to current squad.
            Ok(0) | Err(_) => None,
            Ok(2) => Some(Ok(())),
            Ok(7) => Some(Err("This game is full.".into())),
            Ok(8) => Some(Err(
                "The host's privacy settings prevent joining this game.".into(),
            )),
            Ok(3) => Some(Err("This game is no longer available.".into())),
            Ok(crate::replication_admission::JOIN_REASON_INCOMPATIBLE) => Some(Err(
                "replication_version_mismatch: This game uses an incompatible version.".into(),
            )),
            Ok(4..=6) => Some(Err("Halo 2 could not reach the host's lobby.".into())),
            Ok(_) => Some(Err("The host could not admit you to this game.".into())),
        };
    }

    pub(super) fn observe(self: &Arc<Self>, b: &mut Builder) {
        let entry = self.clone();
        b.raw()
            .observe_before(
                &points::LIVE_JOIN_UPDATE,
                "identify this session join",
                move |o| {
                    if entry.active.load(Ordering::Acquire) {
                        entry.before(&*o.guest(), o.cpu.ecx, o.thread_id);
                    }
                },
            )
            .optional();
        let exit = self.clone();
        b.raw()
            .observe_after(
                &points::LIVE_JOIN_UPDATE,
                "observe the target squad promotion",
                move |o| {
                    if exit.active.load(Ordering::Acquire) {
                        exit.after(&*o.guest(), o.thread_id);
                    }
                },
            )
            .optional();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn only_the_requested_join_can_report_a_promotion_or_failure() {
        let context = 0x2100;
        let mut m = FakeGuest::new();
        m.map(context, 0x200);
        let outcome = Outcome::default();
        outcome.begin([1; 8]);
        m.put(context + 0x3a, &[2; 8]).put_u32(context + 0x104, 2);
        outcome.before(&m, context, 1);
        outcome.after(&m, 1);
        assert_eq!(outcome.take(), None);
        m.put(context + 0x3a, &[1; 8]);
        for (code, result) in [
            (0, None),
            (2, Some(Ok(()))),
            (7, Some(Err("This game is full.".into()))),
            (
                crate::replication_admission::JOIN_REASON_INCOMPATIBLE,
                Some(Err(
                    "replication_version_mismatch: This game uses an incompatible version.".into(),
                )),
            ),
            (
                8,
                Some(Err(
                    "The host's privacy settings prevent joining this game.".into(),
                )),
            ),
        ] {
            m.put_u32(context + 0x104, code);
            outcome.before(&m, context, 1);
            outcome.after(&m, 2);
            assert_eq!(outcome.take(), None);
            outcome.after(&m, 1);
            assert_eq!(outcome.take(), result);
            assert_eq!(outcome.take(), None);
        }
    }
}
