//! The guest draws the Video row and its picker on the reference surface.
//! Record each size the menu observes independently of the viewer's raster.
use super::{Mode, Session, Startup};
use crate::run_opts::NativeSize;
use crate::setting_defs::{GUEST_FRAME, RESOLUTION};
use anyhow::{Result, ensure};
use film::tape::{Call, Tap, VIDEO_MENU_SIZE, VSIZ};
use settings::{Resolution, Store};
use std::sync::Arc;

fn pack((w, h): (u32, u32)) -> Option<u32> {
    Resolution::fixed(w, h).map(|_| w << 16 | h)
}

fn unpack(bits: u32) -> Option<(u32, u32)> {
    let size = (bits >> 16, bits & 0xffff);
    pack(size).map(|_| size)
}

fn observe(tap: &dyn Tap, native: &NativeSize) -> (u32, u32) {
    if tap.replaying() {
        let Some(call) = tap.next(VIDEO_MENU_SIZE, &[], 0) else {
            return GUEST_FRAME;
        };
        if call.outputs.is_empty()
            && let Some(size) = call.result.ok().and_then(unpack)
        {
            return size;
        }
        tap.fault("invalid taped Video menu size".into());
        return GUEST_FRAME;
    }
    let size = native();
    if let Some(bits) = pack(size) {
        tap.record(
            VSIZ,
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
        tap.fault(format!("invalid Video menu size {}x{}", size.0, size.1));
    }
    size
}

impl Session {
    pub fn video_menu_size(
        self: &Arc<Self>,
        native: NativeSize,
        store: &Arc<Store>,
    ) -> Result<NativeSize> {
        let mut state = self.lock();
        if state.header.version < 2 || state.header.meta.source != film::Source::Local {
            return Ok(native);
        }
        if let Mode::Replay(playback, _) = &mut state.mode
            && !Startup::tapes_video_menu(&mut playback.reader)?
        {
            ensure!(
                !playback
                    .reader
                    .machine_section()
                    .entries()
                    .any(|e| e.tag == VSIZ),
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
        fn record(&self, tag: film::Tag, c: Call) {
            assert_eq!(tag, VSIZ);
            self.calls.lock().unwrap().push_back(c)
        }
        fn fault(&self, reason: String) {
            self.failures.lock().unwrap().push(reason);
        }
    }

    #[test]
    fn replay_follows_recorded_resizes_and_rejects_invalid_dimensions() {
        let mut tape = Tape::default();
        let recorded = [(5120, 2880), (3440, 1440)];
        for size in recorded {
            assert_eq!(
                observe(&tape, &(Arc::new(move || size) as NativeSize)),
                size
            );
        }
        let viewer: NativeSize = Arc::new(|| panic!("reading the viewer in an exact replay"));
        tape.replay = true;
        for size in recorded {
            assert_eq!(observe(&tape, &viewer), size);
        }
        for bits in [0, 640 << 16, (8193 << 16) | 480] {
            tape.calls.lock().unwrap().push_back(Call {
                index: 0,
                op: VIDEO_MENU_SIZE.into(),
                args: vec![],
                thread: 0,
                result: Ok(bits),
                outputs: vec![],
            });
            assert_eq!(observe(&tape, &viewer), GUEST_FRAME);
        }
        assert_eq!(tape.failures.lock().unwrap().len(), 3);
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
