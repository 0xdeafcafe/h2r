//! What a ring entry keeps of the Forge monitor looks and poses
//! (docs/game/theater-design.md, "The session ring"): the playback read so
//! far and the tick it stands at, taken with the entry and put back when
//! the entry is loaded, so the monitors shown after a ring jump are the
//! ones the film showed at the entry's tick. The camera's hidden seat is
//! the viewer's and stays.
use super::{STATE, with};
use forge::look::film::Playback;

/// The monitors as an entry keeps them: `None` when nothing had been read.
#[derive(Clone, Debug, Default)]
pub struct Kept(Option<(Playback, u32)>);

/// What a ring entry keeps, taken with the entry.
pub(in crate::theater) fn snapshot() -> Kept {
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    Kept(state.as_ref().map(|s| (s.playback.clone(), s.tick)))
}

/// Put back what [`snapshot`] kept: what was read since goes.
pub(in crate::theater) fn restore(kept: &Kept) {
    match &kept.0 {
        None => super::clear(),
        Some((playback, tick)) => with(|s| {
            s.playback = playback.clone();
            s.tick = *tick;
        }),
    }
}
