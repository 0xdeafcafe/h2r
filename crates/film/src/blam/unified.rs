//! A v3 time chunk. Inputs hold the new values of changed seats; unchanged
//! inputs repeat implicitly until another input record or the chunk end.
use super::{Event, Tick, stream, ticks};
use anyhow::{Context, Result, ensure};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordData {
    pub tag: u8,
    pub payload: Vec<u8>,
}

impl RecordData {
    pub fn stats_event(json: Vec<u8>) -> Self {
        let mut payload = Vec::with_capacity(1 + json.len());
        payload.push(stream::STATS_EVENT);
        payload.extend_from_slice(&json);
        Self {
            tag: stream::EVENT,
            payload,
        }
    }

    pub fn pose(payload: Vec<u8>) -> Self {
        Self {
            tag: stream::POSE,
            payload,
        }
    }

    pub fn marker(label: &str) -> Self {
        Self {
            tag: stream::MARKER,
            payload: label.as_bytes().to_vec(),
        }
    }
}

#[derive(Default, Clone)]
pub(super) struct TickRecords {
    pub during: Vec<RecordData>,
    pub after: Vec<RecordData>,
}

pub(super) fn encode(ticks: &[Tick], extra: &[TickRecords]) -> Result<Vec<u8>> {
    ensure!(
        ticks.len() == extra.len(),
        "record groups disagree with film ticks"
    );
    let first = ticks.first().context("empty time chunk")?.tick;
    let mut out =
        stream::ChunkWriter::new(first, ticks.len().try_into()?).map_err(anyhow::Error::msg)?;
    let mut previous = [None; super::MAX_SEATS];
    for (tick, records) in ticks.iter().zip(extra) {
        let seats = ticks::slots(tick)?;
        if seats != previous {
            let mut input = Vec::new();
            ticks::write_change(&previous, &seats, &mut input);
            out.record(tick.tick, stream::INPUT, &input)
                .map_err(anyhow::Error::msg)?;
            previous = seats;
        }
        lifecycle(&mut out, tick.tick, stream::LIFECYCLE_EVENT, &tick.events)?;
        write_records(&mut out, tick.tick, &records.during)?;
        if let Some(hash) = tick.post_hash {
            out.record(tick.tick, stream::HASH, &hash.to_le_bytes())
                .map_err(anyhow::Error::msg)?;
        }
        write_records(&mut out, tick.tick, &records.after)?;
        lifecycle(
            &mut out,
            tick.tick,
            stream::CLOSING_EVENT,
            &tick.closing_events,
        )?;
    }
    Ok(out.finish())
}

fn write_records(
    writer: &mut stream::ChunkWriter,
    tick: u64,
    records: &[RecordData],
) -> Result<()> {
    for record in records {
        ensure!(
            !matches!(record.tag, stream::INPUT | stream::HASH),
            "duplicate source of engine inputs or hash"
        );
        writer
            .record(tick, record.tag, &record.payload)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

fn lifecycle(
    writer: &mut stream::ChunkWriter,
    tick: u64,
    kind: u8,
    events: &[Event],
) -> Result<()> {
    if !events.is_empty() {
        ticks::validate_events(events)?;
        let mut payload = vec![kind];
        serde_json::to_writer(&mut payload, events)?;
        ensure!(
            payload.len() - 1 <= ticks::MAX_EVENT_BYTES,
            "film lifecycle events too large"
        );
        writer
            .record(tick, stream::EVENT, &payload)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub(super) fn decode(raw: &[u8], first: u64, count: usize) -> Result<VecDeque<Tick>> {
    let chunk = stream::Chunk::parse(raw).map_err(anyhow::Error::msg)?;
    ensure!(
        chunk.first_tick == first && usize::from(chunk.tick_count) == count,
        "film time chunk disagrees with its index"
    );
    let mut records = chunk.records();
    let mut next = records.next().transpose().map_err(anyhow::Error::msg)?;
    let mut frames = VecDeque::with_capacity(count);
    let mut seats = [None; super::MAX_SEATS];
    for index in 0..count {
        // Chunk::parse checked the last tick for overflow.
        let mut frame = Tick {
            tick: first + index as u64,
            ..Default::default()
        };
        let mut input_seen = false;
        let mut lifecycle_seen = [false; 2];
        while let Some(record) = next.filter(|r| r.tick == frame.tick) {
            match record.tag {
                stream::INPUT => {
                    ensure!(!input_seen, "duplicate film input record");
                    let mut payload = record.payload;
                    ticks::read_change(&mut seats, &mut payload)?;
                    ensure!(payload.is_empty(), "trailing bytes in film input");
                    input_seen = true;
                }
                stream::EVENT
                    if matches!(
                        record.payload.first(),
                        Some(&stream::LIFECYCLE_EVENT | &stream::CLOSING_EVENT)
                    ) =>
                {
                    let closing = record.payload[0] == stream::CLOSING_EVENT;
                    let seen = &mut lifecycle_seen[usize::from(closing)];
                    let json = &record.payload[1..];
                    ensure!(
                        json.len() <= ticks::MAX_EVENT_BYTES && !*seen,
                        "invalid film lifecycle record"
                    );
                    let events: Vec<Event> = serde_json::from_slice(json)?;
                    ticks::validate_events(&events)?;
                    if closing {
                        frame.closing_events = events;
                    } else {
                        frame.events = events;
                    }
                    *seen = true;
                }
                stream::HASH => {
                    ensure!(frame.post_hash.is_none(), "duplicate film hash");
                    frame.post_hash = Some(u64::from_le_bytes(record.payload.try_into()?));
                }
                _ => {}
            }
            next = records.next().transpose().map_err(anyhow::Error::msg)?;
        }
        frame.inputs.extend(seats.iter().flatten().copied());
        frames.push_back(frame);
    }
    Ok(frames)
}
