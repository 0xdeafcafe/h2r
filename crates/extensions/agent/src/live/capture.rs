//! Copy a tick out of guest memory. Only the owned wire records cross threads.

use agent_proto::live::{Layers, Object, Player, World};
use guest_layout::{GuestMemory, MemFault};
use sim_state::{ObjectId, ObjectView};

/// Replace the requested layers in a recycled buffer. On a fault the
/// caller discards this sample in full.
pub(crate) fn read(m: &dyn GuestMemory, layers: Layers, out: &mut World) -> Result<(), MemFault> {
    out.objects.clear();
    if layers.objects {
        for object in sim_state::objects(m)? {
            out.objects.push(read_object(object?)?);
        }
    }

    let mut count = 0;
    if layers.players {
        sim_state::visit_players(m, |index, player| {
            if count == out.players.len() {
                out.players.push(Player::default());
            }
            read_player(m, index, player, &mut out.players[count])?;
            count += 1;
            Ok(())
        })?;
    }
    out.players.truncate(count);
    Ok(())
}

fn read_object(view: ObjectView<'_, dyn GuestMemory + '_>) -> Result<Object, MemFault> {
    let transform = view.transform()?;
    let vitality = view.vitality()?;
    Ok(Object {
        id: view.id().0,
        tag: view.definition()?.get(),
        kind: view.kind_byte()?,
        flags: view.flag_bits()?,
        parent: view.parent()?.map(|id| id.0),
        position: transform.position,
        forward: transform.forward,
        up: transform.up,
        scale: transform.scale,
        velocity: view.velocity()?,
        body: vitality.body,
        shield: vitality.shield,
        placement: view.placement()?,
    })
}

fn read_player(
    m: &dyn GuestMemory,
    index: u16,
    player: guest_layout::Player,
    out: &mut Player,
) -> Result<(), MemFault> {
    out.index = index;
    out.team = i16::from(player.team(m)?);
    out.local_user = u16::try_from(player.local_user(m)?)
        .ok()
        .filter(|local| u32::from(*local) < live_telemetry::layout::PLAYER_SLOT_COUNT);

    let units = player.name_units(m)?;
    let len = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    let name = live_proto::bots::unmarked_units(&units[..len]);
    out.name.clear();
    out.name.extend(
        char::decode_utf16(name.iter().copied()).map(|ch| ch.unwrap_or(char::REPLACEMENT_CHARACTER)),
    );

    out.unit = player.unit(m)?.get();
    out.held = None;
    out.aim = [0.0; 3];
    if let Some(id) = out.unit
        && let Some(object) = sim_state::object(m, ObjectId(id))?
        && let Some(unit) = object.as_unit()?
    {
        out.held = unit.held_weapon()?.map(|id| id.0);
        out.aim = unit.aim()?;
    }
    Ok(())
}
