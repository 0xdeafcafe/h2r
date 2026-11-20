/// The same host events enter the real stats projection via each transport.
/// Nonzero match-clock origin and film-clock origin need not coincide.
#[test]
fn imported_tick_events_produce_the_same_stats_as_live_telemetry() {
    use live_proto::film::{events, stream};
    const GUEST_START: u64 = 15_000;
    const FIRST: u64 = 2_001;
    let fixture = [
        (
            0,
            game::GAME_STARTED,
            json!({
                "game_id":"parity", "host":"server:h", "tick":GUEST_START,
                "map":"lockout", "mode":"multiplayer", "variant":"CTF", "team_play":true,
                "players":[{"index":0,"team":0,"name":"Ace","xuid":A},
                           {"index":1,"team":1,"name":"Bee","xuid":B}]
            }),
        ),
        (
            4,
            game::PLAYER_JOINED,
            json!({"player":{"index":2,"team":0,"name":"Cee","xuid":C}}),
        ),
        (
            31,
            game::PLAYER_KILLED,
            json!({"tick":GUEST_START+31,"kind":"kill","killer":0,"victim":1,"weapon":"smg"}),
        ),
        (
            32,
            game::SCORE_CHANGED,
            json!({"tick":GUEST_START+32,"players":[
            {"index":0,"score":1,"kills":1,"deaths":0,"assists":0},
            {"index":2,"score":0,"kills":0,"deaths":0,"assists":1}],"teams":[{"team":0,"score":1}]}),
        ),
        (
            60,
            game::PLAYER_KILLED,
            json!({"tick":GUEST_START+60,"kind":"suicide","victim":0,"killer":0}),
        ),
        (65, game::PLAYER_LEFT, json!({"index":2})),
        (
            90,
            game::GAME_ENDED,
            json!({"tick":GUEST_START+90,"reason":"finished","winner":{"team":0}}),
        ),
    ];
    let mut live = Log::new();
    let mut tracker = events::Tracker::default();
    let mut out = stream::ChunkWriter::new(FIRST, 91).unwrap();
    // Deliberately undecodable as a pose, irrelevant to the stats reader.
    out.record(FIRST, stream::POSE, &[255]).unwrap();
    for (offset, kind, data) in fixture {
        live.push("parity", kind, data.clone(), T0 + offset as i64 / 30);
        for r in tracker.event(kind, &data, GUEST_START + offset).unwrap() {
            let mut payload = vec![stream::STATS_EVENT];
            payload.extend(r.encode().unwrap());
            out.record(FIRST + (r.tick - GUEST_START), stream::EVENT, &payload)
                .unwrap();
        }
    }
    let bytes = out.finish();
    let mut film = Log::new();
    let mut highlights = Vec::new();
    for record in stream::Chunk::parse(&bytes).unwrap().records() {
        let record = record.unwrap();
        if record.tag != stream::EVENT || record.payload.first() != Some(&stream::STATS_EVENT) {
            continue;
        }
        let payload = &record.payload[1..];
        if let Some((tick, kind, data)) = events::Record::decode_telemetry(payload).unwrap() {
            film.push("parity", kind, data, T0 + (tick - GUEST_START) as i64 / 30);
        } else {
            highlights.push(events::Record::decode(payload).unwrap().event);
        }
    }
    assert!(highlights.contains(&events::Event::Assist {
        player: 2,
        count: 1
    }));
    assert!(highlights.contains(&events::Event::Medal {
        player: 0,
        earned_at: GUEST_START + 31,
        medal: "opening-act".into()
    }));
    assert!(highlights.contains(&events::Event::ObjectiveScore {
        game_type: "CTF".into(),
        player: None,
        team: Some(0),
        score: 1
    }));
    let expected = live.stats.summary_at(Some(7), T0 + 100);
    assert_eq!(expected.totals.kills, 1);
    assert_eq!(expected.totals.suicides, 1);
    assert_eq!(row(&expected, A).wins, 1);
    assert_eq!(row(&expected, B).deaths, 1);
    assert_eq!(film.stats.summary_at(Some(7), T0 + 100), expected);
}
