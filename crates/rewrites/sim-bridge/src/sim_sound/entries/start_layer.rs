//! The start layer's bodies over the table's start rows
//! (docs/engine/sound-instances.md, "The start layer").
//!
//! | Entry | Body |
//! |---|---|
//! | `0x21d110` | [`start_record`]: a row for the sound's effect, the sound's record bound to it (`0x21dc60`), the admission, then `Mode::Start` |
//! | `0x21d2c0` | [`tag_start`]: a tag's effect with no sound, then `Mode::Start` |
//! | `0x21d630` | [`run_start`]: [`plan`]'s actions, an admission or a loop call each |
//! | `0x21d360` | [`stop_start`]: stopping, `Mode::Stop`, done |
//! | `0x21d390` | [`starts_update`]: the walk, `Mode::Update` for each row it names |
//!
//! A start's sound that ends or stops queues its start in the table
//! ([`sim_sound::instances::Sounds::take_start_stops`]); [`drain_stops`] runs their stops after
//! the entry that queued them.

use std::collections::BTreeMap;

use engine::entry::{Cx, EntryError};
use engine::sim_sound::gate::{self, Gate};
use engine::sim_sound::{playback, promotion};
use engine::{GuestMemory, MemFault};
use sim_sound::instances::{Handle, SourceParams, TagIndex};
use sim_sound::owners::Owner;
use sim_sound::starts::{Action, Component, DialogueRow, Mode, OwnerPoll, STARTED_TABLE, SOUND_TABLE, Start, StartOf, plan};

use super::{NONE, Shared, lock, object_gone, sound_on, tick_of};
use crate::sim_sound::{guest_tags, request};

/// A request record's bytes (`0x126000` reads to `+0x9f`).
pub const RECORD_SIZE: u32 = 0xa0;
/// The bodies' frame below the entry `ESP`: the record a component's
/// admission builds.
pub const FRAME: u32 = RECORD_SIZE + 0x10;
/// One effect component (group, tag, gain, flags).
const COMPONENT: u32 = 16;

/// An effect's components: count `+8`, rows `+0xC`.
pub fn components(m: &dyn GuestMemory, effect: u32) -> Result<Vec<Component>, MemFault> {
    let count = (m.u32(effect.wrapping_add(8))? as i32).max(0) as u32;
    let rows = m.u32(effect.wrapping_add(0xc))?;
    (0..count)
        .map(|i| {
            let c = rows.wrapping_add(COMPONENT * i);
            Ok(Component {
                group: m.u32(c)?,
                tag: TagIndex(m.u32(c.wrapping_add(4))?),
                gain: m.f32(c.wrapping_add(8))?,
                flags: m.u32(c.wrapping_add(0xc))?,
            })
        })
        .collect()
}

/// `0x21db80`: the source a start's components play from. A tag start's
/// is its scale in word 1 (words 0 byte 3 and 2 zero; retail leaves the
/// rest as stack, the table zeroes it); a sound start's is its sound's.
fn source_of(t: &super::Table, start: &Start) -> SourceParams {
    match start.of {
        StartOf::Tag { scale } => {
            let mut s = SourceParams::default();
            s.0[1] = scale.to_bits();
            s
        }
        StartOf::Sound(sound) => t.sounds.get(sound).map(|i| i.source).unwrap_or_default(),
        StartOf::Dialogue(_) | StartOf::Admitting => SourceParams::default(),
    }
}

/// The stack slot of the frame `at` bytes below the entry `ESP`.
fn slot(cx: &Cx, at: u32) -> u32 {
    cx.entry().esp.wrapping_sub(at)
}

/// `0x21dd30` for `tag` and `custom` ([`playback::effect_of`]): the
/// effect, when it has components. A fault reading the tag data refuses
/// the start as unreadable.
fn effect_of(cx: &Cx, table: &Shared, tag: u32, custom: u32) -> Option<u32> {
    cx.read(|m| playback::effect_of(m, tag, custom)).unwrap_or_else(|_| {
        lock(table).ledger.refused_unreadable += 1;
        None
    })
}

/// The gate (`0x126c30`, [`gate::start_gate`]) and the rule (`0x126ec0`,
/// [`promotion::select`]), then the record read and admitted with the
/// gate's listener as its channel: the handle, or [`NONE`] when the sound
/// is off or any step refuses. A fault reading the gate's or the rule's
/// data refuses the start as unreadable.
pub(super) async fn gate_and_admit(cx: &Cx, table: &Shared, record: u32, tag: u32) -> Result<u32, EntryError> {
    if !cx.read(sound_on) {
        return Ok(NONE);
    }
    Ok(cx.write(|m| {
        let channel = match gate::start_gate(m, record, tag) {
            Ok(Gate { refused: None, listener: Some(listener) }) => listener,
            Ok(_) => return NONE,
            Err(_) => {
                lock(table).ledger.refused_unreadable += 1;
                return NONE;
            }
        };
        let rule = match promotion::select(m, tag) {
            Ok(r) if r.refused => return NONE,
            Ok(r) => r.rule,
            Err(_) => {
                lock(table).ledger.refused_unreadable += 1;
                return NONE;
            }
        };
        let request = request::read(&*m, [tag, channel, record, rule]);
        let tick = tick_of(m).unwrap_or(0);
        lock(table).admit(m, request, tick)
    }))
}

/// `0x21dc60` on the sound's record: the start's table `0x44a1c0` with the
/// start at payload `+0x28` and the record's own table (or 0, the start
/// released) at payload `+0x2c`. The per-owner loop row it also takes
/// (flag bit 4, `0x219a30`, `0x219a90`) is not kept.
fn bind(m: &mut dyn GuestMemory, table: &Shared, record: u32, start: Handle) -> Result<(), MemFault> {
    let flags = m.u32(record)?;
    m.set_u16(record.wrapping_add(0x8c), 0x30)?;
    if flags & 0x20 != 0 {
        let own = m.u32(record.wrapping_add(0x58))?;
        m.set_u32(record.wrapping_add(request::OWN_TABLE_AT), own)?;
    } else {
        lock(table).sounds.start_release(start);
        m.set_u32(record.wrapping_add(request::OWN_TABLE_AT), 0)?;
    }
    m.set_u32(record.wrapping_add(request::START_AT), start.raw())?;
    m.set_u32(record, flags | 0x20)?;
    m.set_u32(record.wrapping_add(0x58), SOUND_TABLE)
}

/// `0x21d110`: a start row when the sound's custom playback row has an
/// effect, the record bound to it, the admission, then the row made a
/// sound start and `Mode::Start` run, or the row freed when the admission
/// refused.
pub(super) async fn start_record(cx: &Cx, table: &Shared) -> Result<u32, EntryError> {
    let record = cx.entry().eax;
    let tag = cx.arg(0)?;
    let flags = cx.read(|m| m.u32(record))?;
    let custom = if flags & (1 << 8) != 0 { cx.read(|m| m.u32(record.wrapping_add(0x98)))? } else { NONE };
    let mut start = None;
    if let Some(effect) = effect_of(cx, table, tag, custom) {
        start = lock(table).sounds.start_new(Start::admitting(effect));
    }
    if let Some(h) = start {
        // `0x18d090`'s row: its byte `+8` bit 0 sets the record's byte
        // `+0xd` bit 0, source word 0 bit 8.
        let bound = cx.write(|m| -> Result<(), MemFault> {
            let row = playback::playback_row(&*m, tag, custom)?;
            if row != 0 && m.u8(row.wrapping_add(8))? & 1 != 0 {
                let byte = m.u8(record.wrapping_add(0xd))?;
                m.set_u8(record.wrapping_add(0xd), byte | 1)?;
            }
            bind(m, table, record, h)
        });
        if bound.is_err() {
            lock(table).ledger.refused_unreadable += 1;
        }
    }
    let sound = gate_and_admit(cx, table, record, tag).await?;
    if let Some(h) = start {
        if sound == NONE {
            lock(table).sounds.start_free(h);
        } else {
            let word = cx.read(|m| m.u16(record.wrapping_add(4))).unwrap_or(0);
            lock(table).sounds.start_attach(h, Handle::from_raw(sound), word);
            run_start(cx, table, h, Mode::Start).await?;
        }
    }
    drain_stops(cx, table).await?;
    Ok(sound)
}

/// `0x21d2c0(ecx = tag, xmm0 = scale, word)`: a tag start, kind 0, when
/// the tag's effect has components; `Mode::Start` runs at once.
pub(super) async fn tag_start(cx: &Cx, table: &Shared) -> Result<u32, EntryError> {
    let e = cx.entry();
    let (tag, scale, word) = (e.ecx, f32::from_bits(e.xmm0 as u32), cx.arg(0)? as u16);
    let Some(effect) = effect_of(cx, table, NONE, tag) else {
        return Ok(NONE);
    };
    let Some(h) = lock(table).sounds.start_new(Start::tag(effect, scale, word)) else {
        return Ok(NONE);
    };
    run_start(cx, table, h, Mode::Start).await?;
    drain_stops(cx, table).await?;
    Ok(h.raw())
}

/// `0x21d360(ecx = start)`: stopping, `Mode::Stop`, done; the next walk
/// frees the row.
pub(super) async fn stop_start(cx: &Cx, table: &Shared, start: Handle) -> Result<(), EntryError> {
    if lock(table).sounds.start_stop_begin(start) {
        run_start(cx, table, start, Mode::Stop).await?;
        lock(table).sounds.start_stop_end(start);
    }
    drain_stops(cx, table).await
}

/// `0x21d630` on its own: the mode as retail's word, 0 to 2.
pub(super) async fn run_entry(cx: &Cx, table: &Shared, start: Handle, mode: u32) -> Result<(), EntryError> {
    let mode = match mode {
        0 => Mode::Start,
        1 => Mode::Update,
        2 => Mode::Stop,
        _ => return Ok(()),
    };
    run_start(cx, table, start, mode).await?;
    drain_stops(cx, table).await
}

/// `0x127f10` for a sound start's sound: its own owner's alive (the unit
/// gone from the objects array), and on a gone owner the class's hold
/// (`0x124f90`). A sound that is not in the table, or that kept no owner,
/// updates.
fn poll(m: &dyn GuestMemory, t: &super::Table, sound: Handle) -> OwnerPoll {
    let running = OwnerPoll { update: true, gone: false };
    let Some(i) = t.sounds.get(sound) else { return running };
    let Owner::Start { own, .. } = i.owner else { return running };
    match own.owner().unit() {
        Some(object) if object_gone(m, object) => {
            OwnerPoll { update: !guest_tags::class_holds(m, i.tag).unwrap_or(false), gone: true }
        }
        _ => running,
    }
}

/// `0x21d390`: the walk, then `Mode::Update` for each row it names.
pub(super) async fn starts_update(cx: &Cx, table: &Shared) -> Result<(), EntryError> {
    let polls: BTreeMap<Handle, OwnerPoll> = cx.read(|m| {
        let t = lock(table);
        t.sounds
            .starts()
            .iter()
            .filter_map(|(h, s)| match s.of {
                StartOf::Sound(sound) if !s.stopping && !s.released => Some((h, poll(m, &t, sound))),
                _ => None,
            })
            .collect()
    });
    let update = lock(table).sounds.starts_walk(&mut |h, _| polls.get(&h).copied().unwrap_or_default());
    for h in update {
        run_start(cx, table, h, Mode::Update).await?;
    }
    drain_stops(cx, table).await
}

/// The stops queued by starts' sounds that ended or stopped: `Mode::Stop`,
/// then done, until none is left.
pub(super) async fn drain_stops(cx: &Cx, table: &Shared) -> Result<(), EntryError> {
    loop {
        let stops = lock(table).sounds.take_start_stops();
        if stops.is_empty() {
            return Ok(());
        }
        for h in stops {
            run_start(cx, table, h, Mode::Stop).await?;
            lock(table).sounds.start_stop_end(h);
        }
    }
}

/// What one run does, read before any of it is done: the plan and the
/// start's source.
fn plan_of(m: &dyn GuestMemory, t: &super::Table, start: Handle, mode: Mode) -> Option<(Vec<Action>, SourceParams)> {
    let s = *t.sounds.starts().get(start)?;
    let parts = components(m, s.effect).ok()?;
    let loop_flags = |tag: TagIndex| -> u32 {
        guest_tags::definition(m, tag.0).ok().filter(|&d| d != 0).and_then(|d| m.u32(d).ok()).unwrap_or(0)
    };
    // No code makes a dialogue start (Not verified): its row is not read.
    let actions = plan(start, &s, mode, &parts, &loop_flags, DialogueRow::default());
    Some((actions, source_of(t, &s)))
}

/// `0x21d630(start, mode)`: each component's admission or loop call, in
/// order. An admission builds the request record in the frame: flags
/// `0x95`, the start's word and source, the gain as the gain base, and
/// for an owned one flag bit 5 with the table `0x44a1e0` and the start at
/// payload `+0x28`.
pub(super) async fn run_start(cx: &Cx, table: &Shared, start: Handle, mode: Mode) -> Result<(), EntryError> {
    let planned = cx.read(|m| {
        let t = lock(table);
        plan_of(m, &t, start, mode)
    });
    let Some((actions, source)) = planned else {
        lock(table).ledger.refused_unreadable += 1;
        return Ok(());
    };
    let record = slot(cx, FRAME);
    for action in actions {
        match action {
            Action::Admit { tag, word, gain, owned } => {
                let wrote = cx.write(|m| -> Result<(), MemFault> {
                    m.write(record, &[0; RECORD_SIZE as usize])?;
                    m.set_u32(record, if owned { 0xb5 } else { 0x95 })?;
                    m.set_u32(record.wrapping_add(4), u32::from(word))?;
                    for (i, w) in source.0.iter().enumerate() {
                        m.set_u32(record.wrapping_add(0xc + 4 * i as u32), *w)?;
                    }
                    if owned {
                        m.set_u32(record.wrapping_add(0x58), STARTED_TABLE)?;
                        m.set_u32(record.wrapping_add(request::START_AT), start.raw())?;
                        m.set_u16(record.wrapping_add(0x8c), 0x30)?;
                    }
                    m.set_f32(record.wrapping_add(0x94), gain)
                });
                if wrote.is_err() {
                    lock(table).ledger.refused_unreadable += 1;
                    continue;
                }
                gate_and_admit(cx, table, record, tag.0).await?;
            }
            Action::Loop { tag, owner, gain, mode, flags, .. } => {
                let mut source = source;
                source.0[2] = (f32::from_bits(source.0[2]) + gain).to_bits();
                let call = super::LoopCall { tag, owner, source, mode, flags, fade: 0.0 };
                cx.write(|m| {
                    let tick = tick_of(m).unwrap_or(0);
                    lock(table).loop_update(m, call, tick)
                });
            }
        }
    }
    Ok(())
}

