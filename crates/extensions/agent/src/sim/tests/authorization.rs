use super::*;

#[test]
fn queued_edits_refuse_a_new_session_or_peer_count() {
    const PEERS: u32 = 0x8170_0000;
    const MODE: u32 = PEERS + 4;
    let (h, q, sim) = with_calls_and_sim();
    let (mut game, _, _) = reload_fixture();
    sim.set_probe(Box::new(|m| SessionView {
        kind: SessionKind::from_sim_mode(m.u32(MODE).unwrap()),
        peers: Some(m.u32(PEERS).unwrap()),
        ..Default::default()
    }));
    for (method, mut params) in edit_requests() {
        params["confirm_peers"] = json!(2);
        for (mode, peers) in [(5, 2), (4, 3)] {
            game.g.put_u32(MODE, 4).put_u32(PEERS, 2);
            h.deferred(&mut game, method, params.clone());
            game.g.put_u32(MODE, mode).put_u32(PEERS, peers);
            let calls = drive_edit_tick(&mut game, &q, |_, _| Default::default());
            assert_eq!(calls.len(), 1, "only the tick for {method}");
            h.agent.on_tick(&mut game);
            assert_eq!(h.reply()["error"]["code"], REFUSED);
        }
    }
}

#[test]
fn queued_edits_refuse_a_live_join_or_replaced_map() {
    use guest_layout::global::{CACHE_MAP_NAME, SCENARIO, SQUAD_SESSION};

    const LIVE: u32 = 0x8170_0000;
    let (h, q) = with_calls();
    h.agent.set_live_session_probe(|m| m.u8(LIVE).unwrap() != 0);
    let (mut game, _, _) = reload_fixture();
    let checksum = CACHE_MAP_NAME.base - 0x198 + 0x2d0;
    for changed in [LIVE, checksum, SCENARIO.base, SQUAD_SESSION.base] {
        game.g.put_u32(LIVE, 0);
        let old = game.g.u32(changed).unwrap();
        h.deferred(
            &mut game,
            method::OBJECT_RESPAWN,
            json!({ "placement": { "block": "vehicles", "index": 0 } }),
        );
        game.g.put_u32(changed, old + 1);
        let calls = drive_edit_tick(&mut game, &q, |_, _| Default::default());
        assert_eq!(calls.len(), 1);
        h.agent.on_tick(&mut game);
        assert_eq!(h.reply()["error"]["code"], REFUSED);
        game.g.put_u32(changed, old);
    }
    h.deferred(&mut game, method::MAP_RELOAD, json!({}));
    // A successful activation can reuse the same cache, addresses and options.
    h.agent.live_cut();
    let calls = drive_edit_tick(&mut game, &q, |_, _| Default::default());
    assert_eq!(calls.len(), 1);
    h.agent.on_tick(&mut game);
    assert_eq!(h.reply()["error"]["code"], REFUSED);
}
