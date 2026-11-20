//! Independently decodable runs of *complete* seat states. No command or
//! dictionary crosses a block boundary, so a seek only inflates one block.
use super::{Event, MAX_SEATS, SeatInput, Tick};
use anyhow::{Context, Result, ensure};
use std::collections::VecDeque;

pub(super) use super::stream::{MAX_TICKS, write_varint as varint};
const MAX_EVENT_BYTES: usize = 64 << 10;

pub(super) fn bytes<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    let (head, rest) = input
        .split_at_checked(len)
        .context("truncated film record")?;
    *input = rest;
    Ok(head)
}

pub(super) fn read_varint(input: &mut &[u8]) -> Result<u64> {
    super::stream::read_varint(input).map_err(anyhow::Error::msg)
}

pub(super) type Seats = [Option<SeatInput>; MAX_SEATS];

pub(super) fn slots(tick: &Tick) -> Result<Seats> {
    let mut slots = [None; MAX_SEATS];
    for &seat in &tick.inputs {
        let slot = slots
            .get_mut(usize::from(seat.seat))
            .context("invalid film seat")?;
        ensure!(
            slot.is_none() && (seat.pad.is_some() || seat.action.is_some()),
            "duplicate or empty film seat"
        );
        *slot = Some(seat);
    }
    Ok(slots)
}

pub(super) fn validate_events(events: &[Event]) -> Result<()> {
    ensure!(events.len() <= 256, "too many events in one tick");
    for e in events {
        let seat = match e {
            Event::Join { player } => player.seat,
            Event::Quit { seat } | Event::Leave { seat } | Event::Team { seat, .. } => *seat,
            Event::HostMigration { .. } | Event::Pause { .. } => continue,
        };
        ensure!(usize::from(seat) < MAX_SEATS, "invalid event seat");
    }
    Ok(())
}

// A set byte bit carries the replacement byte. The baseline of a newly
// present field is zero. This preserves float payloads and padding exactly.
fn encode_field<const N: usize>(old: Option<&[u8; N]>, new: &[u8; N], out: &mut Vec<u8>) {
    let mask = out.len();
    out.resize(mask + N.div_ceil(8), 0);
    for (i, &byte) in new.iter().enumerate() {
        if byte != old.map_or(0, |old| old[i]) {
            out[mask + i / 8] |= 1 << (i % 8);
            out.push(byte);
        }
    }
}

fn decode_field<const N: usize>(old: Option<[u8; N]>, input: &mut &[u8]) -> Result<[u8; N]> {
    let mask = bytes(input, N.div_ceil(8))?;
    if N % 8 != 0 {
        ensure!(mask[mask.len() - 1] >> (N % 8) == 0, "invalid field mask");
    }
    let mut result = old.unwrap_or([0; N]);
    for (i, byte) in result.iter_mut().enumerate() {
        if mask[i / 8] & (1 << (i % 8)) != 0 {
            *byte = bytes(input, 1)?[0];
        }
    }
    Ok(result)
}

pub(super) fn write_change(old: &Seats, new: &Seats, out: &mut Vec<u8>) {
    let mut changed = 0u16;
    for i in 0..MAX_SEATS {
        if old[i] != new[i] {
            changed |= 1 << i;
        }
    }
    varint(changed.into(), out);
    for i in 0..MAX_SEATS {
        if changed & (1 << i) == 0 {
            continue;
        }
        let Some(next) = new[i] else {
            out.push(0);
            continue;
        };
        out.push(u8::from(next.pad.is_some()) | (u8::from(next.action.is_some()) << 1));
        if let Some(pad) = &next.pad {
            encode_field(old[i].as_ref().and_then(|s| s.pad.as_ref()), pad, out);
        }
        if let Some(action) = &next.action {
            encode_field(old[i].as_ref().and_then(|s| s.action.as_ref()), action, out);
        }
    }
}

pub(super) fn read_change(state: &mut Seats, input: &mut &[u8]) -> Result<()> {
    let changed = u16::try_from(read_varint(input)?).context("invalid changed-seat mask")?;
    for (i, slot) in state.iter_mut().enumerate() {
        if changed & (1 << i) == 0 {
            continue;
        }
        let flags = bytes(input, 1)?[0];
        ensure!(flags <= 3, "unknown film input kind");
        let pad = if flags & 1 != 0 {
            Some(decode_field(slot.as_ref().and_then(|s| s.pad), input)?)
        } else {
            None
        };
        let action = if flags & 2 != 0 {
            Some(decode_field(slot.as_ref().and_then(|s| s.action), input)?)
        } else {
            None
        };
        *slot = (flags != 0).then_some(SeatInput {
            seat: i as u8,
            pad,
            action,
        });
    }
    Ok(())
}

pub(super) fn encode(ticks: &[Tick]) -> Result<Vec<u8>> {
    ensure!(
        !ticks.is_empty() && ticks.len() <= MAX_TICKS,
        "invalid film tick block length"
    );
    ensure!(
        ticks.iter().all(|t| t.closing_events.is_empty()),
        "closing events require film v3"
    );
    let mut out = Vec::new();
    varint(ticks[0].tick, &mut out);
    varint(ticks.len() as u64, &mut out);
    let mut old = [None; MAX_SEATS];
    let mut at = 0;
    while at < ticks.len() {
        let next = slots(&ticks[at])?;
        let mut end = at + 1;
        while end < ticks.len() && ticks[end].inputs == ticks[at].inputs {
            end += 1;
        }
        varint((end - at) as u64, &mut out);
        write_change(&old, &next, &mut out);
        old = next;
        at = end;
    }
    for tick in ticks {
        let has_events = !tick.events.is_empty();
        out.push(u8::from(tick.post_hash.is_some()) | (u8::from(has_events) << 1));
        if let Some(hash) = tick.post_hash {
            out.extend_from_slice(&hash.to_le_bytes());
        }
        if has_events {
            validate_events(&tick.events)?;
            let events = serde_json::to_vec(&tick.events)?;
            ensure!(
                events.len() <= MAX_EVENT_BYTES,
                "film event batch too large"
            );
            varint(events.len() as u64, &mut out);
            out.extend(events);
        }
    }
    Ok(out)
}

pub(super) fn decode(mut input: &[u8], first: u64, count: usize) -> Result<VecDeque<Tick>> {
    ensure!(
        count > 0
            && count <= MAX_TICKS
            && read_varint(&mut input)? == first
            && read_varint(&mut input)? == count as u64,
        "film tick block disagrees with its index"
    );
    let mut ticks = VecDeque::with_capacity(count);
    let mut state = [None; MAX_SEATS];
    while ticks.len() < count {
        let duration = usize::try_from(read_varint(&mut input)?)?;
        ensure!(
            duration > 0 && duration <= count - ticks.len(),
            "invalid film input run"
        );
        read_change(&mut state, &mut input)?;
        let inputs: Vec<_> = state.iter().flatten().copied().collect();
        for _ in 0..duration {
            ticks.push_back(Tick {
                tick: first
                    .checked_add(ticks.len() as u64)
                    .context("film tick overflow")?,
                inputs: inputs.clone(),
                ..Tick::default()
            });
        }
    }
    for tick in &mut ticks {
        let flags = bytes(&mut input, 1)?[0];
        ensure!(flags <= 3, "unknown film tick flags");
        if flags & 1 != 0 {
            tick.post_hash = Some(u64::from_le_bytes(bytes(&mut input, 8)?.try_into()?));
        }
        if flags & 2 != 0 {
            let len = usize::try_from(read_varint(&mut input)?)?;
            ensure!(len <= MAX_EVENT_BYTES, "film event batch too large");
            tick.events = serde_json::from_slice(bytes(&mut input, len)?)?;
            validate_events(&tick.events)?;
        }
    }
    ensure!(input.is_empty(), "trailing bytes in film tick block");
    Ok(ticks)
}
