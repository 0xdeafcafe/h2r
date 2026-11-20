//! A roster change can happen on a supervisor slice or in the render hook
//! that refreshes a Clankermatch. Sample both boundaries. The large run of
//! unchanged memberships costs only a counter in the keyframe.
use super::{Mode, Session};
use anyhow::{Result, ensure};
use film::tape::BOTR;
use serde::{Deserialize, Serialize};

const MAX_ROSTER: usize = 1 << 20;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Books {
    local: bots::local::film::Roster,
    remote: bots::phantom::film::Roster,
}
impl Books {
    pub(crate) fn capture() -> Self {
        Self {
            local: bots::local::global().film_roster(),
            remote: bots::phantom::global().film_roster(),
        }
    }
    pub(crate) fn load(&self) -> Result<(), String> {
        bots::local::global().restore_film_roster(&self.local)?;
        bots::phantom::global().restore_film_roster(&self.remote)
    }
}

#[derive(Serialize, Deserialize)]
struct Change {
    boundary: u64,
    books: Books,
}
#[derive(Default)]
pub(super) struct BotTape {
    enabled: bool,
    cursor: u64,
    last: Option<(u64, u64)>,
    next: usize,
    entries: Vec<film::Entry>,
    pending: Option<Change>,
}
impl BotTape {
    pub(super) fn new(mode: &Mode, version: u32) -> Self {
        let mut tape = Self::default();
        if version >= 2
            && let Mode::Replay(play, _) = mode
        {
            tape.entries = play
                .reader
                .machine_section()
                .entries()
                .filter(|e| e.tag == BOTR)
                .cloned()
                .collect();
        }
        tape
    }

    fn apply(&mut self, play: &mut film::tape::Playback) -> Result<()> {
        if self.pending.is_none()
            && let Some(entry) = self.entries.get(self.next)
        {
            let bytes = play.reader.read_bytes_limited(entry, MAX_ROSTER)?;
            self.pending = Some(serde_json::from_slice(&bytes)?);
            self.next += 1;
        }
        if let Some(change) = self.pending.as_ref() {
            ensure!(
                change.boundary >= self.cursor,
                "out of order film bot change"
            );
            if change.boundary == self.cursor {
                change.books.load().map_err(anyhow::Error::msg)?;
                self.pending = None;
            }
        }
        Ok(())
    }
}
impl Session {
    pub(crate) fn has_bot_tape(&self) -> bool {
        self.lock().header.version >= 2
    }
    pub(crate) fn enable_bot_tape(&self) {
        self.bots.lock().unwrap_or_else(|e| e.into_inner()).enabled = true;
    }
    pub(crate) fn bot_cursor(&self) -> u64 {
        self.bots.lock().unwrap_or_else(|e| e.into_inner()).cursor
    }
    pub(crate) fn resume_bot_cursor(&self, cursor: u64) -> Result<(), String> {
        let mut t = self.bots.lock().unwrap_or_else(|e| e.into_inner());
        let offset = self
            .lock()
            .engine
            .as_ref()
            .and_then(|e| e.track.keyframe_offset())
            .ok_or("restoring bot cursor without an indexed keyframe")?;
        t.cursor = cursor;
        t.pending = None;
        t.next = t.entries.partition_point(|e| e.offset <= offset);
        Ok(())
    }
    pub(crate) fn bot_boundary(&self) -> Result<()> {
        let mut t = self.bots.lock().unwrap_or_else(|e| e.into_inner());
        if !t.enabled {
            return Ok(());
        }
        t.cursor = t
            .cursor
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("bot boundary overflow"))?;
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return Ok(());
        }
        match &mut s.mode {
            Mode::Record(rec) => {
                let rev = (
                    bots::local::global().roster_revision(),
                    bots::phantom::global().roster_revision(),
                );
                if t.last != Some(rev)
                    && let Some(out) = &mut rec.out
                {
                    t.last = Some(rev);
                    let change = Change {
                        boundary: t.cursor,
                        books: Books::capture(),
                    };
                    let write = (|| -> Result<()> {
                        let raw = serde_json::to_vec(&change)?;
                        ensure!(raw.len() <= MAX_ROSTER, "bot membership exceeds film limit");
                        out.chunk_bytes(BOTR, &raw)
                    })();
                    if let Err(e) = write {
                        s.fail(format!("recording bot roster: {e:#}"));
                    }
                }
            }
            Mode::Replay(play, _) => {
                if let Err(e) = t.apply(play) {
                    s.fail(format!("restoring bot membership: {e:#}"));
                }
            }
        }
        Ok(())
    }
}
