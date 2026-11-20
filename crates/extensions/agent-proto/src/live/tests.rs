use super::*;

fn header(kind: Kind, seq: u64) -> Header {
    Header {
        kind,
        seq,
        tick: seq * 2,
        epoch: 7,
        cut: false,
        paused: false,
    }
}

const BOTH: Layers = Layers {
    objects: true,
    players: true,
};

fn initial() -> World {
    World {
        objects: vec![Object {
            id: 0x20001,
            tag: Some(66),
            kind: 1,
            flags: 9,
            parent: Some(2),
            position: [1., 2., 3.],
            forward: [0., -1., 0.],
            up: [0., 0., 1.],
            scale: 0.75,
            velocity: [-2., -1., 0.],
            body: 0.4,
            shield: 1.25,
            placement: Some(0),
        }],
        players: vec![Player {
            index: 1,
            name: "Test pilot α".into(),
            unit: Some(0x20001),
            held: None,
            team: -1,
            aim: [0., 0., 1.],
            local_user: Some(0),
        }],
    }
}

fn transfer(frame: Frame, world: &mut World) -> Frame {
    let wire = frame.encode().unwrap();
    let mut stream = wire.as_slice();
    let decoded = Frame::read(&mut stream).unwrap();
    assert!(stream.is_empty());
    decoded.apply(world).unwrap();
    decoded
}

#[test]
fn key_round_trip_clears_old_layers_and_preserves_header() {
    let world = initial();
    let mut hdr = header(Kind::Key, 124);
    hdr.cut = true;
    hdr.paused = true;
    let mut replica = World::default();
    let decoded = transfer(Frame::between(hdr, BOTH, None, &world), &mut replica);
    assert_eq!(decoded.header, hdr);
    assert_eq!(replica, world);
    let players = Layers {
        players: true,
        objects: false,
    };
    transfer(Frame::between(hdr, players, None, &world), &mut replica);
    assert!(replica.objects.is_empty());
    assert_eq!(replica.players, world.players);
}

#[test]
fn delta_contains_only_changed_fields_and_removes_old_salts() {
    let base = initial();
    let mut next = base.clone();
    next.objects[0].position[0] = -0.0;
    next.players[0].held = Some(42);
    let mut replica = base.clone();
    let frame = transfer(
        Frame::between(header(Kind::Delta, 2), BOTH, Some(&base), &next),
        &mut replica,
    );
    assert_eq!(frame.header.kind, Kind::Delta);
    assert_eq!(frame.objects.as_ref().unwrap().rows[0].0, 1 << 4);
    assert_eq!(frame.players.as_ref().unwrap().rows[0].0, 1 << 3);
    let wire = frame.encode().unwrap();
    // A position-only update and a held weapon; the name is key-only.
    assert!(
        !wire
            .windows(base.players[0].name.len())
            .any(|w| w == base.players[0].name.as_bytes())
    );
    assert_eq!(replica, next);
    let mut reuse = next.clone();
    reuse.objects[0].id += 1 << 16;
    reuse.players.clear();
    let frame = transfer(
        Frame::between(header(Kind::Delta, 3), BOTH, Some(&next), &reuse),
        &mut replica,
    );
    assert_eq!(frame.header.kind, Kind::Delta);
    assert_eq!(frame.objects.unwrap().removed, [next.objects[0].id]);
    assert_eq!(replica, reuse);
}

#[test]
fn delta_compares_float_bytes_and_not_float_equality() {
    let mut base = initial();
    base.objects[0].body = f32::from_bits(0x7fc000a1);
    base.objects[0].scale = 0.;
    let mut next = base.clone();
    next.objects[0].scale = -0.;
    let mut replica = base.clone();
    let frame = transfer(
        Frame::between(header(Kind::Delta, 9), BOTH, Some(&base), &next),
        &mut replica,
    );
    assert_eq!(frame.objects.unwrap().rows[0].0, 1 << 7);
    assert_eq!(replica.objects[0].scale.to_bits(), (-0.0f32).to_bits());
    assert_eq!(replica.objects[0].body.to_bits(), 0x7fc000a1);
}

#[test]
fn new_player_and_name_change_force_keys() {
    let base = initial();
    let mut next = base.clone();
    next.players[0].name.push('!');
    for previous in [&World::default(), &base] {
        let mut replica = previous.clone();
        let frame = transfer(
            Frame::between(header(Kind::Delta, 4), BOTH, Some(previous), &next),
            &mut replica,
        );
        assert_eq!(frame.header.kind, Kind::Key);
        assert_eq!(replica, next);
    }
}

#[test]
fn unknown_layer_is_skipped_without_consuming_the_next_frame() {
    let base = initial();
    let frame = Frame::between(header(Kind::Key, 5), BOTH, None, &base);
    let wire = frame.encode().unwrap();
    let mut with_extension = wire.clone();
    with_extension.extend_from_slice(&[99, 3, 0, 0, 0, 8, 7, 6]);
    let size = (with_extension.len() - 4) as u32;
    with_extension[..4].copy_from_slice(&size.to_le_bytes());
    with_extension.extend_from_slice(&wire);
    let mut input = with_extension.as_slice();
    let mut replica = World::default();
    Frame::read(&mut input)
        .unwrap()
        .apply(&mut replica)
        .unwrap();
    assert_eq!(replica, base);
    assert_eq!(input, &wire[..]);
}

#[test]
fn invalid_lengths_are_rejected_before_allocation_or_apply() {
    assert!(Frame::read(&mut u32::MAX.to_le_bytes().as_slice()).is_err());
    let mut bad = Frame::between(header(Kind::Key, 5), BOTH, None, &initial())
        .encode()
        .unwrap();
    // Truncate a player in the second section, leaving a valid outer length.
    bad.pop();
    let size = (bad.len() - 4) as u32;
    bad[..4].copy_from_slice(&size.to_le_bytes());
    assert!(Frame::read(&mut bad.as_slice()).is_err());
}

#[test]
fn rejected_delta_leaves_both_layers_at_the_last_good_sample() {
    let base = initial();
    let mut next = base.clone();
    next.objects[0].position[0] += 1.;
    next.players[0].team = 2;
    let delta = Frame::between(header(Kind::Delta, 2), BOTH, Some(&base), &next);
    let wire = delta.encode().unwrap();
    let delta = Frame::read(&mut wire.as_slice()).unwrap();
    let mut missing_player = base.clone();
    missing_player.players.clear();
    let previous = missing_player.clone();
    assert!(delta.apply(&mut missing_player).is_err());
    assert_eq!(missing_player, previous);
}

#[test]
fn negotiation_reports_unavailable_layers_and_rejects_unknown_options() {
    let params: SubscribeParams = serde_json::from_value(serde_json::json!({
        "layers": ["objects", "poses", "players", "players"]
    }))
    .unwrap();
    assert_eq!((params.every, params.key_every), (1, 150));
    let (layers, refused) = Layers::negotiate(&params.layers);
    assert_eq!(layers.names(), ["objects", "players"]);
    assert_eq!(
        refused.keys().map(String::as_str).collect::<Vec<_>>(),
        ["poses"]
    );
    assert!(
        serde_json::from_value::<SubscribeParams>(serde_json::json!({
            "layers": ["objects"], "evrey": 3
        }))
        .is_err()
    );
}
