//! The guest draws the Video row and its picker on the reference surface.
//! Record each size the menu observes independently of the viewer's raster.
use super::{Mode, Session, Startup};
use crate::run_opts::NativeTarget as ReadTarget;
use crate::setting_defs::{GUEST_FRAME, RESOLUTION};
use anyhow::{Result, ensure};
use film::tape::{Call, Tap, VIDEO_MENU_SIZE};
use settings::{NativeTarget, Resolution, Store};
use std::sync::Arc;

// The picker only observes the viewport. A spare bit marks windowed
// observations, so size-only tapes retain their original budgeted labels.
const WINDOWED: u32 = 1 << 31;

fn pack(target: NativeTarget) -> Option<u32> {
    let (w, h) = target.viewport;
    let mode = if target.fullscreen { 0 } else { WINDOWED };
    Resolution::fixed(w, h).map(|_| mode | (w << 16) | h)
}

fn unpack(bits: u32) -> Option<NativeTarget> {
    let size = ((bits & !WINDOWED) >> 16, bits & 0xffff);
    Resolution::fixed(size.0, size.1)?;
    Some(NativeTarget { fullscreen: bits & WINDOWED == 0, ..NativeTarget::windowed(size) })
}

fn observe(tap: &dyn Tap, native: &ReadTarget) -> NativeTarget {
    if tap.replaying() {
        let Some(call) = tap.next(VIDEO_MENU_SIZE, &[], 0) else {
            return NativeTarget::windowed(GUEST_FRAME);
        };
        if call.outputs.is_empty()
            && let Some(size) = call.result.ok().and_then(unpack)
        {
            return size;
        }
        tap.fault("invalid taped Video menu size".into());
        return NativeTarget::windowed(GUEST_FRAME);
    }
    let size = native();
    if let Some(bits) = pack(size) {
        tap.record(
            Call {
                index: 0,
                op: VIDEO_MENU_SIZE.into(),
                args: vec![],
                thread: 0,
                result: Ok(bits),
                outputs: vec![],
            },
        );
    } else {
        let (w, h) = size.viewport;
        tap.fault(format!("invalid Video menu size {w}x{h}"));
    }
    size
}

impl Session {
    pub fn video_menu_size(
        self: &Arc<Self>,
        native: ReadTarget,
        store: &Arc<Store>,
    ) -> Result<ReadTarget> {
        let mut state = self.lock();
        if state.header.version < 2 || state.header.meta.source != film::Source::Local {
            return Ok(native);
        }
        if let Mode::Replay(playback, _) = &mut state.mode
            && !Startup::tapes_video_menu(&mut playback.reader)?
        {
            ensure!(
                !film::tape::holds_op(&mut playback.reader, VIDEO_MENU_SIZE)?,
                "Video size tape without Video menu startup"
            );
            return Ok(native);
        }
        ensure!(
            self.video_menu.set(store.clone()).is_ok(),
            "duplicate film Video menu"
        );
        let session = self.clone();
        Ok(Arc::new(move || observe(session.as_ref(), &native)))
    }
}

// A seek can jump over a guest Resolution change. The setting and its
// lock come from BOOT; carry its current saved choice across keyframes.
pub(super) fn snapshot(
    store: Option<&Arc<Store>>,
    live: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> hooks::SnapshotPolicy {
    let Some(store) = store else {
        return hooks::SnapshotPolicy::forbid_while(live);
    };
    let (save, load) = (store.clone(), store.clone());
    hooks::SnapshotPolicy::section_forbid_while(
        live,
        move || {
            RESOLUTION
                .name(
                    save.state(RESOLUTION.key)
                        .expect("registered Resolution")
                        .saved,
                )
                .into_bytes()
        },
        move |bytes, _| restore_setting(&load, bytes),
    )
}

fn restore_setting(store: &Store, bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid keyframe Video setting")?;
    let saved = RESOLUTION.parse(text)?;
    let outcome = store
        .set(RESOLUTION.key, saved)
        .map_err(|e| e.to_string())?;
    if outcome.state.saved != saved {
        return Err("keyframe differs from the locked Video setting".into());
    }
    if let Some(error) = outcome.not_saved {
        return Err(error);
    }
    Ok(())
}

/// The picker's setter and frozen option list live in Rust. Defer a seek
/// point across its opening and closing transitions; the next ordinary
/// gameplay keyframe includes any completed change.
pub(super) fn picker_in_stack(guest: &runtime::Guest) -> bool {
    use crate::{layout_glue::Gm, ui_kit::picker, ui_watch};
    if !picker::current().is_some_and(|p| p.title == picker::title(RESOLUTION.label)) {
        return false;
    }
    (0..6).any(|window| {
        (0..=ui_watch::SHARED).any(|slot| {
            ui_watch::stack(guest, window, slot).is_some_and(|s| {
                s.list.iter().any(|(address, _)| {
                    let Some(vtable) = Gm::u32(guest, *address) else {
                        return false;
                    };
                    matches!(
                        crate::list_class::class_name(vtable),
                        Some(crate::video_screen::SCREEN)
                    ) || crate::ui_kit::dialog_tag::by_vtable(vtable)
                        .is_some_and(|d| matches!(d.name, picker::NAME | picker::GAME_NAME))
                })
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Tape {
        replay: bool,
        calls: Mutex<VecDeque<Call>>,
        failures: Mutex<Vec<String>>,
    }
    impl Tap for Tape {
        fn replaying(&self) -> bool {
            self.replay
        }
        fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call> {
            let c = self.calls.lock().unwrap().pop_front()?;
            assert_eq!(
                (c.op.as_str(), c.args.as_slice(), c.thread),
                (op, args, thread)
            );
            Some(c)
        }
        fn record(&self, c: Call) {
            self.calls.lock().unwrap().push_back(c)
        }
        fn fault(&self, reason: String) {
            self.failures.lock().unwrap().push(reason);
        }
    }

    #[test]
    fn replay_follows_recorded_resizes_and_rejects_invalid_dimensions() {
        let mut tape = Tape::default();
        let large = NativeTarget::windowed((5120, 2880));
        let recorded = [
            large,
            NativeTarget { fullscreen: true, ..large },
            NativeTarget::windowed((2784, 1728)),
        ];
        for size in recorded {
            let native: ReadTarget = Arc::new(move || size);
            assert_eq!(observe(&tape, &native), size);
        }
        let viewer: ReadTarget = Arc::new(|| panic!("reading the viewer in an exact replay"));
        tape.replay = true;
        for size in recorded {
            assert_eq!(observe(&tape, &viewer), size);
        }
        for bits in [0, WINDOWED, 640 << 16, (8193 << 16) | 480] {
            tape.calls.lock().unwrap().push_back(Call {
                index: 0,
                op: VIDEO_MENU_SIZE.into(),
                args: vec![],
                thread: 0,
                result: Ok(bits),
                outputs: vec![],
            });
            assert_eq!(observe(&tape, &viewer), NativeTarget::windowed(GUEST_FRAME));
        }
        assert_eq!(tape.failures.lock().unwrap().len(), 4);
    }

    #[test]
    fn keyframe_restores_an_editable_guest_choice() {
        let store = Store::defaults(crate::setting_defs::ALL);
        restore_setting(&store, b"3440x1440").unwrap();
        assert_eq!(
            store.get(RESOLUTION.key),
            Some(Resolution::Fixed(3440, 1440).value())
        );
        assert!(restore_setting(&store, b"9000x1080").is_err());
    }
}
