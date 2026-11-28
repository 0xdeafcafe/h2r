//! Retail bindings for the client. The simulation's entity rows own the
//! association with local objects. The actor channel only reads that association.

use ::actor_net::Control;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::tags::GroupTag;
use guest_layout::view::{Tags, objects};
use guest_layout::{DatumHandle, GuestMemory, MemFault, points};

const DIST_CLIENT: u32 = 5;
const WORLD: u32 = 0x4;
const WORLD_MODE: u32 = 0x8;
const ENTITY_ROWS: u32 = 0x20ac;
const ENTITY_STRIDE: u32 = 0x20;
const ENTITY_SLOT_MASK: u32 = 0x3ff;
const ENTITY_UNIT: u16 = 9;
const OBJECT_DELETING: u8 = 0x10;
const BODY_ENTITY: u32 = 0xd4;
const UNIT_PLAYER: u32 = 0x13c;
const UNIT_CONTROL_MODE: u32 = 0x14c;
const UNIT_FACING: u32 = 0x150;
const UNIT_SPEECH: u32 = 0x342;
const BODY_DEPLETED: u16 = 1 << 2;
const NONE: u32 = u32::MAX;
const CONTROL_WORDS: usize = 0x7c / 4;
const HEAD_MARKER: u32 = 0x0400_0095;
const CROUCHED_POSTURE: u8 = 0x10;

#[derive(Clone, Copy)]
pub(super) struct Unit {
    pub unit: DatumHandle,
    kind: u8,
    body: objects::Body,
}

pub(super) fn client_world(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let globals = m.u32(points::SIM_GLOBALS.base)?;
    if matches!(globals, 0 | NONE) || m.u32(globals.wrapping_add(WORLD_MODE))? != DIST_CLIENT {
        return Ok(None);
    }
    let world = m.u32(globals.wrapping_add(WORLD))?;
    Ok((!matches!(world, 0 | NONE)).then_some(world))
}

pub(super) fn resolve(
    m: &dyn GuestMemory,
    world: Option<u32>,
    entity: u32,
) -> Result<Option<Unit>, MemFault> {
    let Some(world) = world else { return Ok(None) };
    // 0xa58d0 looks up the row and compares the full entity index before
    // returning its local object. 0xa5a96 binds that object and 0xa5ace
    // stamps the entity back into the body. 0xb595a clears the binding.
    let row = world.wrapping_add(ENTITY_ROWS + (entity & ENTITY_SLOT_MASK) * ENTITY_STRIDE);
    if m.u32(row)? != entity || m.u16(row + 4)? != ENTITY_UNIT {
        return Ok(None);
    }
    let Some(unit) = DatumHandle::from_raw(m.u32(row + 8)?) else {
        return Ok(None);
    };
    let Some(pool) = objects::array(m)? else {
        return Ok(None);
    };
    let Some(header) = pool.get(m, unit)? else {
        return Ok(None);
    };
    let kind = header.kind(m)?;
    if !matches!(kind, objects::BIPED | objects::VEHICLE)
        || m.u8(header.addr + 2)? & OBJECT_DELETING != 0
    {
        return Ok(None);
    }
    let body = objects::Body::at(header.body(m)?);
    if body.addr == 0
        || m.u32(body.addr + BODY_ENTITY)? != entity
        || m.u32(body.addr + UNIT_PLAYER)? != NONE
    {
        return Ok(None);
    }
    Ok(Some(Unit { unit, kind, body }))
}

/// The packet carries the low byte of a mode handled by 0x10eef0. Read the
/// complete IDs from that selector's immediate operands in the loaded image.
/// Toggling bit 4 gives each bare mode a distinct crouched code, even
/// when the base code already has bit 4 set. 0xff selects the default.
fn animation_posture(m: &dyn GuestMemory, posture: u8) -> Result<Option<(u32, bool)>, MemFault> {
    const MODES: [(u8, Option<u32>); 9] = [
        (0x84, Some(0x0010_ef65)),
        (0x85, Some(0x0010_ef6d)),
        (0x86, Some(0x0010_ef1d)),
        (0x89, Some(0x0010_ef5e)),
        (0xcb, Some(0x0010_ef47)),
        (0x2c, Some(0x0010_f00a)),
        (0x39, Some(0x0010_f003)),
        (0xc9, Some(0x0010_effc)),
        (0xff, None),
    ];
    for (bare, immediate) in MODES {
        let crouch = posture == bare ^ CROUCHED_POSTURE;
        if posture == bare || crouch {
            let mode = immediate.map_or(Ok(NONE), |address| m.u32(address))?;
            return Ok(Some((mode, crouch)));
        }
    }
    Ok(None)
}

pub(super) fn control_call(
    m: &dyn GuestMemory,
    esp: u32,
    unit: Unit,
    posture: Option<u8>,
    control: Option<Control>,
) -> Result<Option<GuestCall>, MemFault> {
    if (posture.is_none() && control.is_none()) || unit.body.damage_flags(m)? & BODY_DEPLETED != 0 {
        return Ok(None);
    }
    let posture = match posture {
        Some(posture) => animation_posture(m, posture)?,
        None => None,
    };
    if posture.is_none() && control.is_none() {
        return Ok(None);
    }

    // Invert UNIT_APPLY_CONTROL's copies from its input to the unit. Preserve
    // the native packet fields we don't own, including replicated aim, weapon
    // selection and action flags. The grenade selector accepts -1 as no change.
    let body = unit.body.addr;
    let mut packet = [0u8; CONTROL_WORDS * 4];
    for (offset, start, end) in [
        (UNIT_CONTROL_MODE, 0x00, 0x04),
        (0x1bc, 0x04, 0x05),
        (0x214, 0x06, 0x0a),
        (0x241, 0x0c, 0x0d),
        (0x148, 0x10, 0x14),
        (0x1b0, 0x14, 0x20),
        (0x1c0, 0x20, 0x28),
        (UNIT_FACING, 0x28, 0x34),
        (0x15c, 0x34, 0x40),
        (0x180, 0x40, 0x4c),
        (0x1a4, 0x4c, 0x58),
        (0x1c8, 0x58, 0x7c),
    ] {
        m.read(body + offset, &mut packet[start..end])?;
    }
    packet[0x0a..0x0c].fill(0xff);
    if let Some(control) = control {
        packet[0x10] = (packet[0x10] & !1) | u8::from(control.crouch());
        packet[0x14..0x18].copy_from_slice(&control.forward().to_le_bytes());
        packet[0x18..0x1c].copy_from_slice(&control.side().to_le_bytes());
        let [x, y, z] = unit.body.forward(m)?;
        let (sin, cos) = control.facing_delta_radians().sin_cos();
        for (bytes, facing) in
            packet[0x28..0x34]
                .chunks_exact_mut(4)
                .zip([x * cos - y * sin, x * sin + y * cos, z])
        {
            bytes.copy_from_slice(&facing.to_le_bytes());
        }
    }
    if let Some((mode, crouch)) = posture {
        packet[..4].copy_from_slice(&mode.to_le_bytes());
        packet[0x10] = (packet[0x10] & !1) | u8::from(crouch);
    }

    // GuestCall pushes these words directly below the entry stack pointer.
    // UNIT_APPLY_CONTROL reads them through ECX and leaves the stack to its caller.
    Ok(Some(
        GuestCall::new(points::UNIT_APPLY_CONTROL.base)
            .eax(unit.unit.raw())
            .ecx(esp.wrapping_sub(packet.len() as u32))
            .args(
                packet
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .copied()
                    .map(u32::from_le_bytes),
            ),
    ))
}

/// The unit's chosen voice bank and indexed `snd!` reference, as read by
/// 0x1144a0. Peers share the map, so the wire sends a row, never a sound datum.
pub(super) fn line_sound(
    m: &dyn GuestMemory,
    unit: Unit,
    vocalization: u16,
) -> Result<Option<DatumHandle>, MemFault> {
    if vocalization > i16::MAX as u16 {
        return Ok(None);
    }
    let speech = m.u16(unit.body.addr + UNIT_SPEECH)? as i16;
    if speech <= 0 {
        return Ok(None);
    }
    let voice = DatumHandle(m.u32(unit.body.addr + speech as u32)?);
    let Some(bank) = tag_address(m, voice, GroupTag::from_chars(*b"udlg"))? else {
        return Ok(None);
    };
    let index = u32::from(vocalization);
    if index >= m.u32(bank + 0xc)? {
        return Ok(None);
    }
    let rows = m.u32(bank + 0x10)?;
    if rows == 0 {
        return Ok(None);
    }
    let Some(sound) = DatumHandle::from_raw(m.u32(rows + 0x10 * index + 0xc)?) else {
        return Ok(None);
    };
    Ok(tag_address(m, sound, GroupTag::from_chars(*b"snd!"))?.map(|_| sound))
}

fn tag_address(
    m: &dyn GuestMemory,
    datum: DatumHandle,
    group: GroupTag,
) -> Result<Option<u32>, MemFault> {
    let Some(entry) = Tags.entry(m, datum)? else {
        return Ok(None);
    };
    if entry.group(m)? != group {
        return Ok(None);
    }
    Ok(Some(entry.address(m)?).filter(|&address| address != 0))
}

pub(super) async fn play_line(cx: &Cx, unit: Unit, sound: DatumHandle) -> Result<(), EntryError> {
    let sound = sound.raw();
    // One stack argument points to the four parameter words just above it.
    let parameters = cx.entry().esp.wrapping_sub(16);
    let instance = cx
        .call(
            GuestCall::new(points::SOUND_START_AT_MARKER.base)
                .esi(unit.unit.raw())
                .eax(HEAD_MARKER)
                .args([parameters, NONE, sound, 1.0f32.to_bits(), 0]),
        )
        .await?
        .eax;
    if instance != NONE && unit.kind == objects::BIPED {
        cx.call(
            GuestCall::new(points::BIPED_DIALOGUE_LIP_SYNC.base)
                .ebx(unit.unit.raw())
                .args([sound, instance]),
        )
        .await?;
    }
    Ok(())
}
