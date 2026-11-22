/// The same host events enter the real stats projection via each transport.
/// Nonzero match-clock origin and film-clock origin need not coincide. The
/// medals come out the same both ways: the game's own `GameMedal` and the
/// bounce medals, which Live works out from the stats records and not
/// from the film's `Medal` highlights.
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
            33,
            game::GAME_MEDAL,
            json!({"tick":GUEST_START+33,"player":0,"medal":"sniper-kill"}),
        ),
        (
            40,
            game::SUPERBOUNCE,
            json!({"tick":GUEST_START+40,"player":0,"chain":1}),
        ),
        (
            52,
            game::SUPERBOUNCE,
            json!({"tick":GUEST_START+52,"player":0,"chain":2,"monster":true}),
        ),
        (55, game::BOUNCED, json!({"tick":GUEST_START+55,"player":1})),
        (
            60,
            game::PLAYER_KILLED,
            json!({"tick":GUEST_START+60,"kind":"suicide","victim":0,"killer":0}),
        ),
        (
            62,
            game::TEABAG,
            json!({"tick":GUEST_START+62,"player":0,"victim":1}),
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
    // The game's award is a stats record and a highlight; a kill alone
    // awards nothing.
    assert!(highlights.contains(&events::Event::Medal {
        player: 0,
        earned_at: GUEST_START + 33,
        medal: "sniper-kill".into()
    }));
    assert!(!highlights.iter().any(|e| matches!(
        e,
        events::Event::Medal { earned_at, .. } if *earned_at == GUEST_START + 31
    )));
    // A bounce's medals and a bounce death's are highlights; the bounces
    // themselves are stats records.
    assert!(highlights.contains(&events::Event::Medal {
        player: 0,
        earned_at: GUEST_START + 52,
        medal: "double-bounce".into()
    }));
    assert!(highlights.contains(&events::Event::Medal {
        player: 0,
        earned_at: GUEST_START + 52,
        medal: "monster-bounce".into()
    }));
    assert!(highlights.contains(&events::Event::Medal {
        player: 1,
        earned_at: GUEST_START + 55,
        medal: "bounced".into()
    }));
    assert!(!highlights.iter().any(|e| matches!(
        e,
        events::Event::Superbounce(_) | events::Event::Bounced(_) | events::Event::Teabag(_)
    )));
    // Both sides of the teabag lose incel, and Cee left: no one earns it.
    assert!(
        !highlights
            .iter()
            .any(|e| matches!(e, events::Event::Medal { medal, .. } if medal == "incel"))
    );
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
    assert_eq!((expected.totals.bounces, expected.totals.bounced), (2, 1));
    assert_eq!(
        (row(&expected, A).bounces, row(&expected, B).bounced),
        (2, 1)
    );
    assert_eq!(expected.totals.teabags, 1);
    assert_eq!(
        (
            row(&expected, A).teabags_given,
            row(&expected, B).teabags_received
        ),
        (1, 1)
    );
    let medals = |pairs: &[(&str, u64)]| -> BTreeMap<String, u64> {
        pairs.iter().map(|(m, n)| ((*m).to_owned(), *n)).collect()
    };
    assert_eq!(
        expected.totals.medals,
        medals(&[
            ("sniper-kill", 1),
            ("superbounce", 1),
            ("double-bounce", 1),
            ("monster-bounce", 1),
            ("bounced", 1),
        ])
    );
    assert_eq!(
        row(&expected, A).medals,
        medals(&[
            ("sniper-kill", 1),
            ("superbounce", 1),
            ("double-bounce", 1),
            ("monster-bounce", 1),
        ])
    );
    assert_eq!(row(&expected, B).medals, medals(&[("bounced", 1)]));
    assert_eq!(film.stats.summary_at(Some(7), T0 + 100), expected);
}
