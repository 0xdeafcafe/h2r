//! Restore phantom membership, the roster tape cursor and the render hook clock.
use super::*;
use crate::theater::{Session, bot_tape::Books};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct State {
    books: Books,
    cursor: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_roster: Option<usize>,
    frame: u64,
}

pub(super) fn install(b: &mut Builder, frame: &Arc<AtomicU64>, session: &Arc<Session>) {
    let (save, load) = (frame.clone(), frame.clone());
    let (rec, play) = (session.clone(), session.clone());
    b.snapshot(hooks::SnapshotPolicy::section(
        move || {
            if !crate::theater::in_keyframe() {
                return Vec::new();
            }
            let (cursor, next_roster) = rec.bot_snapshot_cursor();
            serde_json::to_vec(&State {
                books: Books::capture(),
                cursor,
                next_roster,
                frame: save.load(Ordering::Relaxed),
            })
            .expect("fixed film bot state is serialisable")
        },
        move |bytes, _| {
            if !crate::theater::in_keyframe() {
                return if bytes.is_empty() {
                    Ok(())
                } else {
                    Err("bot state requires film playback".into())
                };
            }
            let state: State = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            state.books.load()?;
            play.resume_bot_row(state.cursor, state.next_roster)?;
            load.store(state.frame, Ordering::Relaxed);
            Ok(())
        },
    ));
}
