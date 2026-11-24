//! Recorded animations, written (docs/editor/rich-editors.md,
//! "Recordings", slice K3): a stream parsed into events that encode back
//! to the same bytes, the edits on it, and the block's edits as the byte
//! patches a save writes.
//!
//! [`Stream`] keeps what [`crate::recording::decode_stream`] drops (the
//! initial state's padding, the form each delay was written in, and any
//! bytes after the end event), so `parse` then `encode` gives the stream
//! back byte for byte. An edit keeps every event that is not named at the
//! tick it was at: removing one hands its delay to the next, and
//! inserting one takes its delay from the next.
//!
//! [`changes`] turns the block's entries as edited into what a save
//! writes: element bytes that differ, data written in place when it is no
//! longer than the source's, and a stream that outgrew its source as a
//! [`Grown`] to be appended to the metadata (the scenario session's
//! `TagGrown`). Adding a recording is not supported: a block keeps the
//! element room it has, and fewer elements is a removal.

use crate::recording::{
    self, Block, ELEMENT, END, Game, MAX_STREAM, Recorded, TRAILER, Value, WalkError,
    handler_count, initial_fields, initial_value, payload_len, prologue_len,
};

/// Where an element's stream size sits (its address follows).
pub const PAIR: usize = 0x2c;
/// Where an element's tick count sits.
pub const TICKS: usize = 0x24;

/// One event of a stream, as it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ev {
    pub kind: u8,
    /// Ticks before the event.
    pub delay: u16,
    /// Header and delay bytes as written (1, 2 or 3): kept so an
    /// unedited stream encodes to itself. A delay that does not fit is
    /// written in the smallest form that holds it.
    pub head_len: usize,
    pub payload: Vec<u8>,
}

/// A stream read for editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream {
    pub game: Game,
    pub control_version: u8,
    /// The initial state and the twelve trailer bytes, as they are.
    pub prologue: Vec<u8>,
    /// Every event, the end event last.
    pub events: Vec<Ev>,
    /// Bytes after the end event, kept.
    pub trailing: Vec<u8>,
}

fn smallest_head(delay: u16) -> usize {
    match delay {
        0 | 1 => 1,
        2..=255 => 2,
        _ => 3,
    }
}

impl Stream {
    /// Frame `bytes` as `game`'s player does (an error is
    /// [`recording::walk`]'s).
    pub fn parse(game: Game, control_version: u8, bytes: &[u8]) -> Result<Self, WalkError> {
        let w = recording::walk(game, control_version, bytes)?;
        let prologue = bytes[..prologue_len(game, control_version)].to_vec();
        let events = w
            .events
            .iter()
            .map(|e| Ev {
                kind: e.kind,
                delay: e.delay,
                head_len: e.head_len,
                payload: bytes[e.at + e.head_len..e.at + e.size()].to_vec(),
            })
            .collect();
        let end = w.events.last().map_or(prologue.len(), |e| e.at + e.size());
        Ok(Self {
            game,
            control_version,
            prologue,
            events,
            trailing: bytes[end..].to_vec(),
        })
    }

    /// The stream's bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.prologue.clone();
        for e in &self.events {
            let head = e.head_len.clamp(1, 3).max(smallest_head(e.delay));
            let form = match head {
                1 => e.delay as u8,
                2 => 2,
                _ => 3,
            };
            out.push(e.kind << 2 | form);
            match head {
                2 => out.push(e.delay as u8),
                3 => out.extend_from_slice(&e.delay.to_le_bytes()),
                _ => {}
            }
            out.extend_from_slice(&e.payload);
        }
        out.extend_from_slice(&self.trailing);
        out
    }

    /// Ticks to the end event.
    pub fn ticks(&self) -> u32 {
        self.events.iter().map(|e| u32::from(e.delay)).sum()
    }

    fn event(&self, i: usize) -> Result<&Ev, String> {
        self.events
            .get(i)
            .ok_or_else(|| format!("no event {i} (the stream has {})", self.events.len()))
    }

    /// Set event `i`'s payload bytes (the type's length, no other).
    pub fn set_payload(&mut self, i: usize, payload: &[u8]) -> Result<(), String> {
        let kind = self.event(i)?.kind;
        check_payload(self.game, kind, payload)?;
        self.events[i].payload = payload.to_vec();
        Ok(())
    }

    /// Move event `i` to `delay` ticks after the one before it. Every
    /// event after it keeps its tick (the next one's delay takes the
    /// difference); the end event has none after it, so the stream gets
    /// longer or shorter.
    pub fn set_delay(&mut self, i: usize, delay: u16) -> Result<(), String> {
        let old = self.event(i)?.delay;
        if let Some(next) = self.events.get(i + 1) {
            let n = i32::from(next.delay) + i32::from(old) - i32::from(delay);
            let n = u16::try_from(n).map_err(|_| {
                format!(
                    "event {i} cannot be {delay} ticks after the one before it: the next event \
                     would have to come {} ticks earlier than it can",
                    -n.min(0)
                )
            })?;
            self.events[i + 1].delay = n;
        }
        self.events[i].delay = delay;
        Ok(())
    }

    /// Insert an event of `kind` before event `i` (which may be the end
    /// event), `delay` ticks after the one before it. It takes its delay
    /// from the event it goes before, which must have that much.
    pub fn insert(&mut self, i: usize, kind: u8, delay: u16, payload: &[u8]) -> Result<(), String> {
        if i >= self.events.len() {
            return Err(format!(
                "index {i}: events go before the end event, which is {}",
                self.events.len() - 1
            ));
        }
        if kind == END {
            return Err("a stream has one end event".into());
        }
        check_payload(self.game, kind, payload)?;
        let before = self.events[i].delay;
        let rest = before.checked_sub(delay).ok_or_else(|| {
            format!("delay {delay} is past the {before} ticks before the event it goes in front of")
        })?;
        self.events[i].delay = rest;
        self.events.insert(
            i,
            Ev {
                kind,
                delay,
                head_len: smallest_head(delay),
                payload: payload.to_vec(),
            },
        );
        Ok(())
    }

    /// Remove event `i`; its delay goes to the next event, so the rest
    /// stay at their ticks. The end event cannot go.
    pub fn remove(&mut self, i: usize) -> Result<(), String> {
        let e = self.event(i)?;
        if e.kind == END {
            return Err("the end event cannot be removed".into());
        }
        let delay = e.delay;
        let next = &mut self.events[i + 1];
        next.delay = next
            .delay
            .checked_add(delay)
            .ok_or("the next event would be more than 65535 ticks after the one before it")?;
        self.events.remove(i);
        Ok(())
    }

    /// Set the initial state's field `name` (`animation_state`,
    /// `throttle`, ...; the names `decode_stream` gives).
    pub fn set_initial(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let mut at = 0;
        for (field, len) in initial_fields(self.game, self.control_version) {
            let probe = initial_value(field, &vec![0; len]);
            if probe.as_ref().is_some_and(|p| p.0 == name) {
                let (_, shape) = probe.unwrap();
                let bytes = match shape {
                    Value::Reals(r) => reals(value, r.len())?,
                    Value::Signed(_) => int(value, len, true)?,
                    Value::Unsigned(_) => int(value, len, false)?,
                };
                self.prologue[at..at + len].copy_from_slice(&bytes);
                return Ok(());
            }
            at += len;
        }
        Err(format!("no initial field {name:?}"))
    }

    /// Set the twelve trailer bytes.
    pub fn set_trailer(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() != TRAILER {
            return Err(format!(
                "the trailer is {TRAILER} bytes, not {}",
                bytes.len()
            ));
        }
        let n = self.prologue.len();
        self.prologue[n - TRAILER..].copy_from_slice(bytes);
        Ok(())
    }

    /// Whether the stream can be written as it is: every event has a
    /// handler and the payload lengths are the types'. The edits keep
    /// this; it is the check behind a write.
    pub fn check(&self) -> Result<(), String> {
        let bytes = self.encode();
        if bytes.len() > MAX_STREAM {
            return Err(format!(
                "the stream is {} bytes, past {MAX_STREAM}",
                bytes.len()
            ));
        }
        recording::walk(self.game, self.control_version, &bytes)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn check_payload(game: Game, kind: u8, payload: &[u8]) -> Result<(), String> {
    let want = match kind {
        recording::DELAY | END => 0,
        _ => payload_len(game, kind).ok_or_else(|| {
            format!(
                "type {kind} has no handler (the types are 0 to {})",
                handler_count(game) - 1
            )
        })?,
    };
    if payload.len() != want {
        return Err(format!(
            "type {kind} takes {want} payload bytes, not {}",
            payload.len()
        ));
    }
    Ok(())
}

/// `value` as a little-endian integer of `width` bytes.
pub fn int(value: &Value, width: usize, signed: bool) -> Result<Vec<u8>, String> {
    let n: i64 = match value {
        Value::Unsigned(u) => i64::from(*u),
        Value::Signed(s) => i64::from(*s),
        Value::Reals(_) => return Err("an integer is wanted, not reals".into()),
    };
    let bits = 8 * width as u32;
    let (lo, hi) = if signed {
        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
    } else {
        (0, (1i64 << bits) - 1)
    };
    if n < lo || n > hi {
        return Err(format!("{n} does not fit {lo}..={hi}"));
    }
    Ok(n.to_le_bytes()[..width].to_vec())
}

fn reals(value: &Value, count: usize) -> Result<Vec<u8>, String> {
    match value {
        Value::Reals(r) if r.len() == count && r.iter().all(|f| f.is_finite()) => {
            Ok(r.iter().flat_map(|f| f.to_le_bytes()).collect())
        }
        Value::Reals(r) if r.len() == count => Err("a real is not finite".into()),
        _ => Err(format!("{count} finite reals are wanted")),
    }
}

/// The payload of an event of `kind` from named values, as
/// [`recording::decode_stream`] names them: `value` for types 2 to 6 and
/// 23, `a` and `b` for the deltas (7 to 22; `channel` is the type's and
/// is not taken).
pub fn payload_from(kind: u8, fields: &[(&str, Value)]) -> Result<Vec<u8>, String> {
    let get = |n: &str| {
        fields
            .iter()
            .find(|f| f.0 == n)
            .map(|f| &f.1)
            .ok_or_else(|| format!("type {kind} needs {n:?}"))
    };
    match kind {
        2 => int(get("value")?, 1, false),
        3 => int(get("value")?, 1, true),
        4 => int(get("value")?, 4, false),
        5 | 23 => int(get("value")?, 2, true),
        6 => reals(get("value")?, 2),
        7..=14 => Ok([int(get("a")?, 1, true)?, int(get("b")?, 1, true)?].concat()),
        15..=22 => Ok([int(get("a")?, 2, true)?, int(get("b")?, 2, true)?].concat()),
        _ => Err(format!(
            "type {kind} has no values to set (its payload is empty)"
        )),
    }
}

/// One recording of the block as edited: the element it came from, and
/// the stream and tick count it has now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The source element's `ELEMENT` bytes, whose size and address words
    /// point at the source's stream.
    pub element: Vec<u8>,
    /// The source's stream.
    pub original: Vec<u8>,
    pub stream: Vec<u8>,
    pub ticks: i16,
}

impl Entry {
    pub fn name(&self) -> String {
        let end = self.element[..32]
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(32);
        String::from_utf8_lossy(&self.element[..end]).into_owned()
    }

    /// The row this entry reads as.
    pub fn recorded(&self) -> Recorded {
        Recorded {
            name: self.name(),
            version: self.element[0x20],
            control_version: self.element[0x22],
            ticks: self.ticks,
            stream: self.stream.clone(),
        }
    }
}

/// The block's entries as the source has them.
pub fn entries(block: &Block) -> Vec<Entry> {
    block
        .elements
        .iter()
        .zip(&block.rows)
        .map(|(element, r)| Entry {
            element: element.clone(),
            original: r.stream.clone(),
            stream: r.stream.clone(),
            ticks: r.ticks,
        })
        .collect()
}

/// A stream that outgrew its source: its bytes go to the end of the
/// metadata and the `(size, address)` pair at `root` points there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grown {
    pub root: u32,
    pub bytes: Vec<u8>,
}

/// What a save writes for the block as edited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockChanges {
    /// `(guest address, bytes)`, in place, ascending.
    pub patches: Vec<(u32, Vec<u8>)>,
    pub grown: Vec<Grown>,
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// The patches that turn `source` into `entries`: the elements in the
/// order given (a removal is the later ones moved down and the count
/// lowered). Each entry's element goes to its slot: bytes that differ are
/// patched; a stream no longer than its source's is written over it and
/// the size patched, a longer one is [`Grown`] at its slot's pair; the
/// old streams are left as dead bytes. More entries than the source had
/// elements is refused.
pub fn changes(source: &Block, entries: &[Entry]) -> Result<BlockChanges, String> {
    if entries.len() > source.elements.len() {
        return Err(format!(
            "the block has room for {} recordings, not {}: adding one is not supported",
            source.elements.len(),
            entries.len()
        ));
    }
    let mut out = BlockChanges::default();
    for (i, e) in entries.iter().enumerate() {
        if e.element.len() != ELEMENT {
            return Err(format!(
                "recording {i}: the element is {} bytes",
                e.element.len()
            ));
        }
        if e.stream.len() > MAX_STREAM {
            return Err(format!(
                "recording {}: stream past {MAX_STREAM} bytes",
                e.name()
            ));
        }
        let at = source.address + (i * ELEMENT) as u32;
        let (size0, addr0) = (word(&e.element, PAIR) as usize, word(&e.element, PAIR + 4));
        let mut image = e.element.clone();
        image[TICKS..TICKS + 2].copy_from_slice(&e.ticks.to_le_bytes());
        if e.stream.len() <= size0 {
            if e.stream != e.original && !e.stream.is_empty() {
                out.patches.push((addr0, e.stream.clone()));
            }
            if e.stream.len() != e.original.len() {
                image[PAIR..PAIR + 4].copy_from_slice(&(e.stream.len() as u32).to_le_bytes());
            }
        } else {
            // The slot's pair is written by the save, once the copy has
            // its address: the element keeps the slot's source words.
            let slot = &source.elements[i];
            image[PAIR..].copy_from_slice(&slot[PAIR..]);
            out.grown.push(Grown {
                root: at + PAIR as u32,
                bytes: e.stream.clone(),
            });
        }
        diff_into(&mut out.patches, at, &image, &source.elements[i]);
    }
    if entries.len() != source.elements.len() {
        out.patches
            .push((source.root, (entries.len() as u32).to_le_bytes().to_vec()));
    }
    out.patches.sort();
    Ok(out)
}

/// One patch per run of bytes that differ between `after` and `before`
/// (the element at `at`).
fn diff_into(out: &mut Vec<(u32, Vec<u8>)>, at: u32, after: &[u8], before: &[u8]) {
    let mut i = 0;
    while i < after.len() {
        if after[i] == before[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < after.len() && after[i] != before[i] {
            i += 1;
        }
        out.push((at + start as u32, after[start..i].to_vec()));
    }
}

#[cfg(test)]
#[path = "recording_edit_tests.rs"]
mod tests;
