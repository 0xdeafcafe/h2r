//! The exact or interpolated records presented by the editor this frame.
use agent_proto::live::{Object, Player};
use live_mirror::Sample;

fn same_object(a: &Object, b: &Object) -> bool {
    a.id == b.id
        && a.tag == b.tag
        && a.kind == b.kind
        && a.flags == b.flags
        && a.parent == b.parent
        && a.placement == b.placement
        && a.position.map(f32::to_bits) == b.position.map(f32::to_bits)
        && a.forward.map(f32::to_bits) == b.forward.map(f32::to_bits)
        && a.up.map(f32::to_bits) == b.up.map(f32::to_bits)
        && a.scale.to_bits() == b.scale.to_bits()
        && a.velocity.map(f32::to_bits) == b.velocity.map(f32::to_bits)
        && a.body.to_bits() == b.body.to_bits()
        && a.shield.to_bits() == b.shield.to_bits()
}

fn same_player(a: &Player, b: &Player) -> bool {
    a.index == b.index
        && a.name == b.name
        && a.team == b.team
        && a.unit == b.unit
        && a.held == b.held
        && a.local_user == b.local_user
        && a.aim.map(f32::to_bits) == b.aim.map(f32::to_bits)
}

/// Wire floats include signed zero and NaN payloads. Equal payloads have
/// the same displayed revision, including for a headless viewport sample.
pub(crate) fn same_records(sample: &Sample, objects: &[Object], players: &[Player]) -> bool {
    sample.objects.len() == objects.len()
        && sample.players.len() == players.len()
        && sample
            .objects
            .iter()
            .zip(objects)
            .all(|(a, b)| same_object(a, b))
        && sample
            .players
            .iter()
            .zip(players)
            .all(|(a, b)| same_player(a, b))
}
