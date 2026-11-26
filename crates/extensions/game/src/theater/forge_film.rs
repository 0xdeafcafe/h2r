//! Forge monitor looks and poses at film playback (docs/game/
//! forge-monitor-appearance.md, "Theater", ML8). The film's Forge entry
//! records (`live_proto::film::stream::FORGE`) of subtype look and pose are
//! presentation: the track reader hands them over with the tick that holds
//! them ([`deliver`]), and they build the `Playback` the camera rig, the
//! bodies the renderer draws and the Theater bar read. The welcome and the
//! stream items are the session's edit log, which the simulation's own
//! state already carries, so playback does not replay them here.
//!
//! The state is a host static, like `firefight::film`'s: one film plays at a
//! time. A keyframe restore clears it and the film's own checkpoint records
//! rebuild it ([`clear`]); a ring restore puts back what its entry held
//! ([`Snapshot`]).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use agent_proto::theater::CameraMode;
use anyhow::{Result, anyhow};
use forge::look::body::{Bodies, Draw};
use forge::look::film::{self as looks, Playback, Subject, Unseen, View};
use forge::look::session::CanDraw;

/// What playback has read of the film's monitor records so far, the film
/// tick it stands at and the seat a first-person camera looks from.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    playback: Playback,
    tick: u32,
    hide: Option<u8>,
}

static STATE: Mutex<Option<Snapshot>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Snapshot) -> R) -> R {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(state.get_or_insert_with(Snapshot::default))
}

/// A film opens or a keyframe is restored: what was read goes, and the
/// records that follow (a chunk's checkpoint first) rebuild it.
pub fn clear() {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// What a ring entry keeps, to put back when it is restored.
#[allow(dead_code)] // Film playback does not call it yet: Forge edits in a film are recorded but not replayed.
pub(super) fn snapshot() -> Snapshot {
    with(|s| s.clone())
}

/// Put back what [`snapshot`] kept.
#[allow(dead_code)] // Film playback does not call it yet: Forge edits in a film are recorded but not replayed.
pub(super) fn restore(snapshot: &Snapshot) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot.clone());
}

/// Read the monitor records of the film tick `tick`, which the playback now
/// stands at. A record that is not readable is a named film error.
#[allow(dead_code)] // Film playback does not call it yet: Forge edits in a film are recorded but not replayed.
pub(super) fn deliver(tick: u64, payloads: &[Vec<u8>]) -> Result<()> {
    with(|s| {
        s.tick = u32::try_from(tick).unwrap_or(u32::MAX);
        for payload in payloads {
            if matches!(payload.first(), Some(&looks::LOOK | &looks::POSE)) {
                s.playback
                    .feed(payload)
                    .map_err(|e| anyhow!("The film's monitor record is not readable: {e}"))?;
            }
        }
        Ok(())
    })
}

/// Whether the film has named any monitor seat's look yet: whether there
/// are bodies to draw.
pub fn active() -> bool {
    with(|s| !s.playback.table().is_empty())
}

/// Whether `seat` is a monitor the film has spoken of: a camera may name it
/// though the roster does not.
pub fn is_monitor(seat: u8) -> bool {
    with(|s| s.playback.is_monitor(seat))
}

/// How the camera takes a monitor seat in `mode`; the other modes do not.
pub fn view_of(mode: CameraMode) -> Option<View> {
    match mode {
        CameraMode::Follow => Some(View::Follow),
        CameraMode::Orbit => Some(View::Orbit),
        CameraMode::FirstPerson => Some(View::FirstPerson),
        CameraMode::Default | CameraMode::Free => None,
    }
}

/// What a camera on monitor `seat` follows now. A first-person camera has
/// the seat's own body hidden from the draws.
pub fn subject(seat: u8, view: View) -> Option<Subject> {
    with(|s| {
        let subject = s.playback.subject(seat, s.tick, view);
        s.hide = subject.filter(|sub| !sub.body).map(|sub| sub.seat);
        subject
    })
}

/// No camera is on a monitor seat: every body is drawn.
pub fn release() {
    with(|s| s.hide = None);
}

/// The bodies drawn now: the live session's own code, `Bodies::draw`, at
/// the film's tick. `rgb` reads a palette index from the loaded map.
pub fn draws(bodies: &Bodies, rgb: impl Fn(u8) -> [u8; 3]) -> Vec<Draw> {
    with(|s| s.playback.draws(bodies, s.tick, s.hide, rgb))
}

/// The looks this machine cannot draw, by seat, from what has been read.
pub fn unseen(can: &CanDraw) -> Vec<Unseen> {
    with(|s| s.playback.unseen(can))
}

/// The monitors a session ring entry keeps and puts back.
pub(super) mod ring;

/// A film file's look records, found by one read of its track.
type Looks = Vec<Vec<u8>>;

static SCANS: Mutex<Option<HashMap<PathBuf, (u64, SystemTime, Looks)>>> = Mutex::new(None);

/// The look records a film file holds: each distinct one, in film order.
/// The track is read once and the answer kept until the file changes, as a
/// film's compatibility is asked for again and again.
fn look_records(path: &Path) -> Result<Looks> {
    let meta = std::fs::metadata(path)?;
    let stamp = (meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    if let Some((len, modified, found)) = SCANS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|scans| scans.get(path))
        && (*len, *modified) == stamp
    {
        return Ok(found.clone());
    }
    let mut track = film::blam::TrackReader::open_with_film(path)?;
    let found = distinct_looks(|| track.step())?;
    SCANS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(path.to_owned(), (stamp.0, stamp.1, found.clone()));
    Ok(found)
}

/// The distinct look records of the ticks `step` gives, in film order. A
/// chunk's checkpoint repeats the table, so the same record comes again.
fn distinct_looks(mut step: impl FnMut() -> Result<Option<film::blam::Tick>>) -> Result<Looks> {
    let mut found: Looks = Vec::new();
    while let Some(tick) = step()? {
        for payload in tick.forge {
            if payload.first() == Some(&looks::LOOK) && !found.contains(&payload) {
                found.push(payload);
            }
        }
    }
    Ok(found)
}

/// The looks the film at `path` records that this machine cannot draw
/// (`monitor_look_source`). A film that cannot be read has none to name:
/// its own errors are the film's.
pub fn unseen_in(path: &Path, can: &CanDraw) -> Vec<Unseen> {
    let Ok(records) = look_records(path) else {
        return Vec::new();
    };
    unseen_of(&records, can)
}

/// The looks among `records` this machine cannot draw.
fn unseen_of(records: &[Vec<u8>], can: &CanDraw) -> Vec<Unseen> {
    let mut playback = Playback::default();
    for record in records {
        // A record that is not readable is the film's error; the rest count.
        let _ = playback.feed(record);
    }
    playback.unseen(can)
}

#[cfg(test)]
mod tests;
