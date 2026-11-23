//! Firefight in a film (docs/game/firefight.md, "Theater"): each host-log
//! entry a machine applies is a film record
//! (`live_proto::film::stream::FIREFIGHT`), its canonical JSON, at the tick
//! it was applied. A film's playback runs the mode as a replica
//! (`runtime::role`), so the director is never stepped there: the
//! theater hands each film tick's records over when the tick before it
//! completes ([`deliver`]), and the runtime's next pre-tick takes them
//! ([`take_played`]) into the replica's log, which applies them at their
//! ticks. The mode blob rides in its `ModeState` entries, so the film draws
//! the host's HUD.

use std::collections::VecDeque;
use std::sync::Mutex;

use ::firefight::Command;
use host_log::Entry;

/// Entries recorded and not yet taken by a film at most. A film takes
/// them every tick, so only a machine recording no film reaches it, and it
/// keeps the latest.
pub const MAX_WAITING: usize = 4096;

static RECORDED: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static PLAYED: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// One entry's record payload.
pub fn payload(entry: &Entry<Command>) -> Result<Vec<u8>, host_log::Error> {
    entry.canonical_bytes()
}

/// The entry a record payload holds.
pub fn parse(payload: &[u8]) -> Result<Entry<Command>, String> {
    serde_json::from_slice(payload).map_err(|e| format!("a film Firefight record: {e}"))
}

/// Keep the entries applied this tick for the film being recorded.
/// Returns the entries that could not be encoded, which no film gets.
pub fn record<'a>(entries: impl IntoIterator<Item = &'a Entry<Command>>) -> Vec<String> {
    let mut r = RECORDED.lock().unwrap_or_else(|e| e.into_inner());
    let mut failed = Vec::new();
    for entry in entries {
        match payload(entry) {
            Ok(bytes) => keep(&mut r, bytes),
            Err(e) => failed.push(format!("host log entry {} for the film: {e}", entry.seq)),
        }
    }
    failed
}

/// Add `bytes`, the oldest going past [`MAX_WAITING`].
fn keep(waiting: &mut Vec<Vec<u8>>, bytes: Vec<u8>) {
    if waiting.len() >= MAX_WAITING {
        waiting.remove(0);
    }
    waiting.push(bytes);
}

/// The record payloads kept since the last call, in the order applied
/// (`theater::record`).
pub fn take() -> Vec<Vec<u8>> {
    std::mem::take(&mut *RECORDED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A film tick's records, handed over by the playback before the tick.
pub fn deliver(payloads: &[Vec<u8>]) {
    PLAYED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(payloads.iter().cloned());
}

/// The records handed over since the last call, in film order.
pub fn take_played() -> Vec<Vec<u8>> {
    PLAYED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain(..)
        .collect()
}

/// A game starts: what the last one left goes.
pub fn clear() {
    take();
    take_played();
}

#[cfg(test)]
mod tests;
