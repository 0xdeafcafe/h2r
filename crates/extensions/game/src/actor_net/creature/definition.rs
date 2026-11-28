//! Slot-17 entity callbacks. The generic object adapter supplies the
//! existence checks and deletion path. The probe supplies the shape,
//! payload, placement and position callbacks those routines dispatch to.

use guest_layout::{GuestMemory, MemFault, points, view::objects};
use guest_layout::tags::{GroupTag, entry_layout, index_layout};
use hooks::{BuildError, Builder, Cx, EntryFuture, GuestCall, Ret};

use super::wire::{
    CREATION_BITS, CREATION_BYTES, Creation, POSITION_BYTES, decode_position, encode_position,
    fail_creation_write, read_payload, write_payload,
};
use super::{
    BODY_ENTITY, CLIENT, NAME, NO_ENTITY, PLACEMENT, POSITION, RECORD_OBJECT, SLOT, body,
    count_update, counter_at, creature_record, database, init_counter, live_object, mode,
    movement_bits,
};

const CREATURE_GROUP: GroupTag = GroupTag::from_chars(*b"crea");

/// Resolve a wire index in this map's local tag table and require the
/// concrete group. The host's salted handle is never sent.
pub(super) fn local_creature_tag(m: &dyn GuestMemory, index: u16) -> Result<Option<u32>, MemFault> {
    let Some(header) = points::TAG_INDEX.pointer(m)? else {
        return Ok(None);
    };
    if index > i16::MAX as u16 || i32::from(index) >= m.i32(header + index_layout::TAG_COUNT)? {
        return Ok(None);
    }
    let Some(entries) = points::TAG_ENTRIES.pointer(m)? else {
        return Ok(None);
    };
    let entry = entries + u32::from(index) * entry_layout::STRIDE;
    let datum = m.u32(entry + entry_layout::DATUM)?;
    if datum == NO_ENTITY
        || datum as u16 != index
        || m.u32(entry + entry_layout::GROUP)? != CREATURE_GROUP.0
        || m.u32(entry + entry_layout::ADDRESS)? == 0
    {
        return Ok(None);
    }
    Ok(Some(datum))
}

pub(super) fn capture(
    m: &dyn GuestMemory,
    body: objects::Body,
) -> Result<Option<Creation>, MemFault> {
    let Some(tag) = body.definition(m)? else {
        return Ok(None);
    };
    let index = tag.index();
    if local_creature_tag(m, index)? != Some(tag.raw()) {
        return Ok(None);
    }
    let creation = Creation {
        definition: index,
        position: body.position(m)?,
        forward: body.forward(m)?,
        up: body.up(m)?,
    };
    Ok(creation.valid().then_some(creation))
}

fn capture_into(
    m: &mut dyn GuestMemory,
    object: u32,
    size: u32,
    data: u32,
) -> Result<bool, MemFault> {
    if size != CREATION_BYTES as u32 {
        return Ok(false);
    }
    let Some(body) = body(m, object)? else {
        return Ok(false);
    };
    let Some(creation) = capture(m, body)? else {
        return Ok(false);
    };
    m.write(data, &creation.encode())?;
    Ok(true)
}

fn creation_at(m: &dyn GuestMemory, size: u32, data: u32) -> Result<Option<Creation>, MemFault> {
    if size != CREATION_BYTES as u32 {
        return Ok(None);
    }
    let mut bytes = [0; CREATION_BYTES];
    m.read(data, &mut bytes)?;
    Ok(Creation::decode(bytes))
}

fn collect(
    m: &mut dyn GuestMemory,
    object: u32,
    mask: u32,
    size: u32,
    state: u32,
) -> Result<u32, MemFault> {
    if size != POSITION_BYTES as u32 || mask & POSITION == 0 {
        return Ok(0);
    }
    let Some(body) = body(m, object)? else {
        return Ok(0);
    };
    let position = body.position(m)?;
    if !position.into_iter().all(f32::is_finite) || position == m.vec3(state)? {
        return Ok(0);
    }
    m.write(state, &encode_position(position))?;
    Ok(POSITION)
}

fn write_creation(cx: &Cx) -> Result<u32, MemFault> {
    let size = cx.arg(0)?;
    let data = cx.arg(1)?;
    let stream = cx.arg(3)?;
    cx.write(|m| {
        if let Some(creation) = creation_at(m, size, data)?
            && write_payload(m, stream, creation.encode())?
        {
            return Ok(0);
        }
        fail_creation_write(m, stream)?;
        Ok(0)
    })
}

fn read_creation(cx: &Cx) -> Result<u32, MemFault> {
    let size = cx.arg(0)?;
    let data = cx.arg(1)?;
    let stream = cx.arg(2)?;
    if size != CREATION_BYTES as u32 {
        return Ok(0);
    }
    cx.write(|m| {
        let Some(creation) = read_payload(m, stream, Creation::decode)? else {
            return Ok(0);
        };
        if local_creature_tag(m, creation.definition)?.is_none() {
            return Ok(0);
        }
        m.write(data, &creation.encode())?;
        Ok(1)
    })
}

fn build_baseline(cx: &Cx) -> Result<u32, MemFault> {
    let creation_size = cx.arg(0)?;
    let creation_data = cx.arg(1)?;
    let state_size = cx.arg(2)?;
    let state = cx.arg(3)?;
    if state_size != POSITION_BYTES as u32 {
        return Ok(0);
    }
    cx.write(|m| {
        let Some(creation) = creation_at(m, creation_size, creation_data)? else {
            return Ok(0);
        };
        m.write(state, &encode_position(creation.position))?;
        Ok(1)
    })
}

fn write_update(cx: &Cx) -> Result<u32, MemFault> {
    let mask = cx.arg(1)?;
    let emitted = cx.arg(2)?;
    let state_size = cx.arg(3)?;
    let state = cx.arg(4)?;
    let stream = cx.arg(6)?;
    cx.write(|m| {
        m.set_u32(emitted, 0)?;
        if mask != POSITION || state_size != POSITION_BYTES as u32 {
            return Ok(0);
        }
        let position = m.vec3(state)?;
        if !position.into_iter().all(f32::is_finite)
            || !write_payload(m, stream, encode_position(position))?
        {
            return Ok(0);
        }
        m.set_u32(emitted, POSITION)?;
        Ok(1)
    })
}

fn read_update(cx: &Cx) -> Result<u32, MemFault> {
    let emitted = cx.arg(1)?;
    let state_size = cx.arg(2)?;
    let state = cx.arg(3)?;
    let stream = cx.arg(4)?;
    if state_size != POSITION_BYTES as u32 {
        return Ok(0);
    }
    cx.write(|m| {
        let Some(position) = read_payload(m, stream, decode_position)? else {
            return Ok(0);
        };
        m.write(state, &encode_position(position))?;
        m.set_u32(emitted, POSITION)?;
        Ok(1)
    })
}

fn create_object(cx: Cx, placement: u32) -> EntryFuture {
    Box::pin(async move {
        let size = cx.arg(0)?;
        let data = cx.arg(1)?;
        let mask = cx.arg(2)?;
        let state_size = cx.arg(3)?;
        let state = cx.arg(4)?;
        let local = cx.read(|m| {
            let Some(creation) = creation_at(m, size, data)? else {
                return Ok::<_, MemFault>(None);
            };
            let Some(tag) = local_creature_tag(m, creation.definition)? else {
                return Ok(None);
            };
            let flags = m.u32(mask)?;
            if state_size != POSITION_BYTES as u32 || flags & !POSITION != 0 {
                return Ok(None);
            }
            let position = if flags == POSITION {
                m.vec3(state)?
            } else {
                creation.position
            };
            Ok(position.into_iter().all(f32::is_finite).then_some((
                tag,
                position,
                creation.forward,
                creation.up,
            )))
        })?;
        let Some((tag, position, forward, up)) = local else {
            return Ok(Ret::new(NO_ENTITY, 20));
        };
        cx.call(
            GuestCall::new(points::CREATURE_PROBE_PLACEMENT_NEW.base)
                .esi(placement)
                .args([tag, NO_ENTITY, 0]),
        )
        .await?;
        cx.write(|m| {
            // The object adapter's generic placement helper marks replicas
            // with these two placement flags before calling object_new.
            let flags = m.u32(placement + 0x18)?;
            m.set_u32(placement + 0x18, flags | 0x12)?;
            m.write(placement + 0x1c, &encode_position(position))?;
            m.write(placement + 0x28, &encode_position(forward))?;
            m.write(placement + 0x34, &encode_position(up))
        })?;
        let object = cx
            .call(GuestCall::new(points::CREATURE_PROBE_OBJECT_NEW.base).args([placement]))
            .await?
            .eax;
        if object != NO_ENTITY {
            cx.call(
                GuestCall::new(points::CREATURE_PROBE_REPLICA_PHYSICS.base)
                    .edx(1)
                    .args([object]),
            )
            .await?;
            // The initial position was applied at placement. The generic
            // adapter only dispatches an update for the remaining mask.
            cx.write(|m| m.set_u32(mask, 0))?;
        }
        Ok(Ret::new(object, 20))
    })
}

fn create_replica(cx: Cx, definition: u32) -> EntryFuture {
    Box::pin(async move {
        let record = cx.arg(0)?;
        let result = cx
            .call(
                GuestCall::new(points::SIM_OBJECT_CREATE_REPLICA.base)
                    .ecx(cx.entry().ecx)
                    .args([
                        record,
                        cx.arg(1)?,
                        cx.arg(2)?,
                        cx.arg(3)?,
                        cx.arg(4)?,
                        cx.arg(5)?,
                    ]),
            )
            .await?;
        if result.eax & 0xff != 0 {
            cx.write(|m| {
                let entity = m.u32(record)?;
                let object = m.u32(record + RECORD_OBJECT)?;
                init_counter(m, definition, entity, object)
            })?;
        }
        Ok(Ret::new(result.eax, 24))
    })
}

fn apply_position(cx: Cx, definition: u32) -> EntryFuture {
    Box::pin(async move {
        let object = cx.arg(0)?;
        let mask = cx.arg(1)?;
        let size = cx.arg(2)?;
        let state = cx.arg(3)?;
        let entity = cx.read(|m| {
            if mask != POSITION
                || size != POSITION_BYTES as u32
                || mode(m) != Some(CLIENT)
                || !m.vec3(state).ok()?.into_iter().all(f32::is_finite)
            {
                return None;
            }
            let body = body(m, object).ok()??;
            let entity = m.u32(body.addr + BODY_ENTITY).ok()?;
            let record = creature_record(m, database(m)?, entity)?;
            (live_object(m, record, entity) == Some(object)).then_some(entity)
        });
        if let Some(entity) = entity {
            cx.call(
                GuestCall::new(points::CREATURE_PROBE_OBJECT_POSITION.base)
                    .eax(state)
                    .ecx(0)
                    .edx(0)
                    .args([object, 0, 0]),
            )
            .await?;
            cx.write(|m| count_update(m, definition, entity, object))?;
        }
        Ok(Ret::new(0, 16))
    })
}

fn entry(
    b: &mut Builder,
    label: &'static str,
    pop: u16,
    run: impl Fn(&Cx) -> Result<u32, MemFault> + Send + Sync + 'static,
) -> Result<u32, BuildError> {
    b.entry(label, move |cx| -> EntryFuture {
        let result = run(&cx);
        Box::pin(async move { Ok(Ret::new(result?, pop)) })
    })
}

/// Fill the probe's slots in a copy of the generic object vtable. Every
/// inherited slot operates only on the entity-to-object association or
/// returns a constant. Entries which address payloads use our own layouts.
pub(super) fn entries(b: &mut Builder, definition: u32) -> Result<Vec<(u32, u32)>, BuildError> {
    let type_at = entry(b, "type", 0, |_| Ok(SLOT))?;
    let name_at = entry(b, "name", 0, move |_| Ok(definition + NAME))?;
    let state_at = entry(b, "state size", 0, |_| Ok(POSITION_BYTES as u32))?;
    let creation_at = entry(b, "creation size", 0, |_| Ok(CREATION_BYTES as u32))?;
    let count_at = entry(b, "flag count", 0, |_| Ok(1))?;
    let initial_at = entry(b, "initial mask", 0, |_| Ok(POSITION))?;
    let creation_bits = entry(b, "creation bits", 12, |cx| {
        let out = cx.arg(2)?;
        cx.write(|m| m.set_u32(out, 5 + CREATION_BITS))?;
        Ok(0)
    })?;
    let describe = entry(b, "describe creation", 16, |cx| {
        let buffer = cx.arg(2)?;
        let cap = cx.arg(3)? as usize;
        if cap != 0 {
            let description = b"creature creation";
            let len = description.len().min(cap - 1);
            cx.write(|m| {
                m.write(buffer, &description[..len])?;
                m.set_u8(buffer + len as u32, 0)
            })?;
        }
        Ok(0)
    })?;
    let update_bits = entry(b, "update bits", 12, |cx| {
        let context = cx.arg(1)?;
        let out = cx.arg(2)?;
        cx.write(|m| {
            let mask = m.u32(context)?;
            m.set_u32(out, movement_bits(mask))
        })?;
        Ok(0)
    })?;
    let encode_creation = entry(b, "write creation", 16, write_creation)?;
    let decode_creation = entry(b, "read creation", 12, read_creation)?;
    let encode_update = entry(b, "write position", 32, write_update)?;
    let decode_update = entry(b, "read position", 20, read_update)?;
    let compare_state = entry(b, "compare position state", 12, |cx| {
        let first = cx.arg(1)?;
        let second = cx.arg(2)?;
        cx.read(|m| Ok(u32::from(m.vec3(first)? == m.vec3(second)?)))
    })?;
    let capture_entity = entry(b, "capture new creature", 12, move |cx| {
        let record = cx.arg(0)?;
        let size = cx.arg(1)?;
        let data = cx.arg(2)?;
        cx.write(|m| {
            let object = m.u32(record + RECORD_OBJECT)?;
            if capture_into(m, object, size, data)? {
                let entity = m.u32(record)?;
                init_counter(m, definition, entity, object)?;
            }
            Ok(0)
        })
    })?;
    let baseline = entry(b, "build position baseline", 16, build_baseline)?;
    let collect_entity = entry(b, "collect entity movement", 16, |cx| {
        let record = cx.arg(0)?;
        let flags = cx.arg(1)?;
        let size = cx.arg(2)?;
        let state = cx.arg(3)?;
        cx.write(|m| {
            let object = m.u32(record + RECORD_OBJECT)?;
            let mask = m.u32(flags)?;
            let emitted = collect(m, object, mask, size, state)?;
            m.set_u32(flags, emitted)?;
            Ok(1)
        })
    })?;
    let refresh = entry(b, "refresh the creature identity", 4, move |cx| {
        let record = cx.arg(0)?;
        cx.write(|m| {
            let entity = m.u32(record)?;
            let object = m.u32(record + RECORD_OBJECT)?;
            if let Some(body) = body(m, object)? {
                m.set_u32(body.addr + BODY_ENTITY, entity)?;
                let counter = counter_at(definition, entity);
                if m.u32(counter + 4)? == object {
                    m.set_u32(counter, entity)?;
                }
            }
            Ok(0)
        })
    })?;
    let create = b.entry("create and bind a creature replica", move |cx| {
        create_replica(cx, definition)
    })?;
    let capture_object = entry(b, "capture creature object creation", 12, |cx| {
        let object = cx.arg(0)?;
        let size = cx.arg(1)?;
        let data = cx.arg(2)?;
        cx.write(|m| Ok(u32::from(capture_into(m, object, size, data)?)))
    })?;
    let collect_object = entry(b, "collect creature object movement", 16, |cx| {
        let object = cx.arg(0)?;
        let mask = cx.arg(1)?;
        let size = cx.arg(2)?;
        let state = cx.arg(3)?;
        cx.write(|m| collect(m, object, mask, size, state))
    })?;
    let place = b.entry("place the local creature replica", move |cx| {
        create_object(cx, definition + PLACEMENT)
    })?;
    let apply = b.entry("apply a creature position", move |cx| {
        apply_position(cx, definition)
    })?;
    let no_capture_physics = entry(b, "skip physics snapshot", 12, |_| Ok(0))?;
    let no_apply_physics = entry(b, "skip physics restoration", 12, |_| Ok(0))?;
    Ok(vec![
        (0x00, type_at),
        (0x04, name_at),
        (0x08, state_at),
        (0x0c, creation_at),
        (0x10, count_at),
        (0x14, initial_at),
        (0x24, creation_bits),
        (0x28, describe),
        (0x2c, update_bits),
        (0x30, encode_creation),
        (0x34, decode_creation),
        (0x38, encode_update),
        (0x3c, decode_update),
        (0x40, compare_state),
        (0x48, capture_entity),
        (0x4c, baseline),
        (0x50, collect_entity),
        (0x54, refresh),
        (0x58, create),
        (0x68, capture_object),
        (0x6c, collect_object),
        (0x74, place),
        (0x7c, apply),
        (0x88, no_capture_physics),
        (0x8c, no_apply_physics),
    ])
}
