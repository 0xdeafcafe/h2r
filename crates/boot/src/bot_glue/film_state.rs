//! The original hook clocks and signed-in accounts govern which guest
//! calls bot management makes after loading an isolated film machine.
use super::*;
use crate::theater::{Session, bot_tape::Books};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct State {
    books: Books,
    cursor: u64,
    frame: u64,
    live_every: u64,
    sign_in_every: u64,
    offline_ids: [Option<[u8; guest::ID_LEN]>; 4],
    pressed: bool,
}

pub(super) fn install(b: &mut Builder, bots: &Bots, session: &Arc<Session>) {
    let (check, save, load) = (bots.clone(), bots.clone(), bots.clone());
    let (rec, play) = (session.clone(), session.clone());
    b.snapshot(feature::SnapshotPolicy::section_forbid_while(
        move || {
            if !crate::theater::in_keyframe() {
                return None;
            }
            (check.signing_out.load(Ordering::Relaxed)
                || lock(&check.render).is_some()
                || lock(&check.sound).is_some()
                || lock(&check.listeners).is_some()
                || lock(&check.owners).is_some()
                || lock(&check.panes).is_some())
            .then(|| "bot guest call or hidden local player map is active".into())
        },
        move || {
            if !crate::theater::in_keyframe() {
                return Vec::new();
            }
            serde_json::to_vec(&State {
                books: Books::capture(),
                cursor: rec.bot_cursor(),
                frame: save.frame.load(Ordering::Relaxed),
                live_every: save.live_every.0.load(Ordering::Relaxed),
                sign_in_every: save.sign_in_every.0.load(Ordering::Relaxed),
                offline_ids: *lock(&save.offline_ids),
                pressed: save.pressed.load(Ordering::Relaxed),
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
            play.resume_bot_cursor(state.cursor)?;
            load.frame.store(state.frame, Ordering::Relaxed);
            load.live_every.0.store(state.live_every, Ordering::Relaxed);
            load.sign_in_every
                .0
                .store(state.sign_in_every, Ordering::Relaxed);
            load.pressed.store(state.pressed, Ordering::Relaxed);
            *lock(&load.offline_ids) = state.offline_ids;
            Ok(())
        },
    ));
}
