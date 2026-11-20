//! Negotiated live records. Every key is a complete v7 key, which seeds the
//! prediction columns. Probability tables always start from the same priors.
use super::*;
mod columns;
mod literals;
mod projection;
mod range;
use columns::{History, META, Symbols};
use literals::*;
use range::{ByteReader, ByteWriter, Tables, Tape};

#[derive(Default)]
pub(super) struct Stream {
    history: History,
    tables: Tables,
}
struct Seed;
impl Symbols for Seed {
    const WRITING: bool = true;
    fn bit(&mut self, _: usize, value: bool) -> bool {
        value
    }
    fn uint(&mut self, _: usize, value: u64) -> u64 {
        value
    }
    fn literal(&mut self, value: u8) -> u8 {
        value
    }
}

impl Stream {
    fn seed(&mut self, frame: &Frame, seq: u64) -> Result<()> {
        self.history
            .write_frame(&mut Seed, frame, &Lists::of(frame)?, 0)?;
        self.history.tick = frame.tick;
        self.history.seq = seq + 1;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Writer {
    stream: Stream,
    projection: projection::Projection,
}

impl Writer {
    pub(super) fn encode(
        &mut self,
        f: &Frame,
        sid: u64,
        seq: u64,
        force: bool,
        baseline: Option<&BTreeMap<u16, MachinePosition>>,
    ) -> Result<(Vec<u8>, bool)> {
        let h = &self.stream.history;
        let key = force
            || seq == 0
            || f.cut.is_some()
            || f.tick < h.tick
            || f.game_id != h.meta.0
            || h.strings.len() >= MAX_STRINGS;
        let mut bytes = Vec::with_capacity(128);
        bytes.push(8 | (u8::from(key) << 4) | ((sid.min(7) as u8) << 5));
        if sid >= 7 {
            put(&mut bytes, sid - 7);
        }
        put(&mut bytes, seq);
        if key {
            let (key_bytes, _) = Encoder::new(0, 1).encode_with_machine_baseline(f, baseline)?;
            let canonical = decode_keyframe(&key_bytes)?;
            let mut stream = Stream::default();
            stream.seed(&canonical, seq)?;
            self.projection.get(f)?;
            self.stream = stream;
            bytes.extend_from_slice(&key_bytes);
        } else {
            let dt = f.tick - h.tick;
            put(&mut bytes, dt);
            let (src, held, _) = self.projection.get(f)?;
            let h = &mut self.stream.history;
            h.held_lists = held;
            let mut r = Tape {
                engine: ByteWriter::new(&mut bytes),
                t: &mut self.stream.tables,
            };
            h.write_frame(&mut r, f, src, dt)?;
            r.engine.finish();
            h.seq = seq + 1;
            h.tick = f.tick;
        }
        Ok((bytes, key))
    }
}

pub(super) fn decode(decoder: &mut Decoder, mut data: &[u8]) -> Result<Decoded> {
    let (&head, rest) = data.split_first().ok_or(PackError::Short)?;
    data = rest;
    let key = head & 16 != 0;
    let sid = match head >> 5 {
        7 => take(&mut data)?
            .checked_add(7)
            .ok_or(PackError::Bad("v8 stream id overflow"))?,
        v => u64::from(v),
    };
    let seq = take(&mut data)?;
    if seq == u64::MAX {
        return Err(PackError::Bad("v8 sequence overflow"));
    }
    if key
        && !decoder.streams.contains_key(&sid)
        && !decoder.causal.contains_key(&sid)
        && decoder.streams.len() + decoder.causal.len() >= MAX_STREAMS
    {
        return Err(PackError::Bad("too many streams"));
    }
    let (frame, state) = if key {
        if data.first().map(|b| b & 31) != Some(0x17) {
            return Err(PackError::Bad("expected embedded v7 key"));
        }
        let d = Decoder::new().decode(data)?;
        if d.sid != 0 || d.seq != 0 {
            return Err(PackError::Bad("invalid embedded key header"));
        }
        let mut state = Stream::default();
        state.seed(&d.frame, seq)?;
        (d.frame, state)
    } else {
        let mut state = decoder
            .causal
            .remove(&sid)
            .ok_or(PackError::Gap { sid, seq })?;
        if state.history.seq != seq {
            return Err(PackError::Gap { sid, seq });
        }
        let dt = take(&mut data)?;
        let mut r = Tape {
            engine: ByteReader::new(data)?,
            t: &mut state.tables,
        };
        let f = state.history.read_frame(&mut r, dt, seq)?;
        r.engine.finished()?;
        (f, state)
    };
    decoder.streams.remove(&sid);
    decoder.causal.insert(sid, state);
    Ok(Decoded {
        sid,
        seq,
        keyframe: key,
        machine_baseline: key,
        frame,
    })
}
