use super::*;
use live_proto::{
    film::{events, stream},
    game,
};
use std::collections::BTreeMap;

/// Exercise the production observer, completed tick writer, teardown flush
/// and front-of-file range reader. Ben's last-word is announced after he
/// left and Cara took his seat. The match clock starts long after boot.
#[test]
fn sealed_menu_medals_match_the_recorded_awards_across_a_seat_replacement() {
    let dir = std::env::temp_dir().join(format!("film-medals-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("match.h2film");
    let session = Session::open(
        Some(file.clone()),
        None,
        host_facts(),
        b"xbe",
        &options(&dir),
    )
    .unwrap()
    .unwrap();
    let observer = session.film_observer().unwrap();
    let guest = arena();
    let emit = |kind, tick, data| {
        let event = crate::telemetry::GameEvent::new("fixture", kind, data);
        observer.event(&event, tick).unwrap();
    };
    emit(
        game::GAME_STARTED,
        1000,
        json!({
            "host":"fixture","game_id":"fixture","tick":1000,"map":"lockout","mode":"multiplayer",
            "variant":"Slayer","team_play":false,"players":[
                {"index":0,"name":"Ada","team":0},{"index":1,"name":"Ben","team":0}]
        }),
    );
    session.checkpoint(&guest);
    for (tick, killer, victim) in [(1005, 0, 1), (1010, 0, 1), (1020, 1, 0)] {
        emit(
            game::PLAYER_KILLED,
            tick,
            json!({
                "tick":tick,"killer":killer,"victim":victim,"kind":"kill","weapon":"smg"
            }),
        );
        session.checkpoint(&guest);
    }
    emit(game::PLAYER_LEFT, 1022, json!({"index":1}));
    emit(
        game::PLAYER_JOINED,
        1022,
        json!({"player":{"index":1,"name":"Cara","team":0}}),
    );
    session.checkpoint(&guest);
    // No further engine tick. Last-word must survive in the closing records.
    emit(
        game::GAME_ENDED,
        1050,
        json!({"tick":1050,"reason":"finished","players":[]}),
    );
    session.finish(120, true).unwrap();

    let mut reader = film::range::RangeReader::new(std::fs::File::open(&file).unwrap()).unwrap();
    let summary: BTreeMap<_, _> = reader
        .front
        .summary
        .players
        .iter()
        .map(|p| (p.name.clone(), p.medals.clone()))
        .collect();
    let mut slot = BTreeMap::new();
    let mut killers = BTreeMap::new();
    let mut emitted = BTreeMap::<String, BTreeMap<String, u32>>::new();
    for i in 0..reader.directory.chunks.len() {
        let raw = reader.chunk(i).unwrap();
        for r in stream::Chunk::parse(&raw).unwrap().records() {
            let r = r.unwrap();
            if r.tag != stream::EVENT || r.payload.first() != Some(&stream::STATS_EVENT) {
                continue;
            }
            let event = events::Record::decode(&r.payload[1..]).unwrap();
            match event.event {
                events::Event::GameStarted(s) => {
                    for p in s.game.players {
                        emitted.entry(p.name.clone()).or_default();
                        slot.insert(p.index, p.name);
                    }
                }
                events::Event::PlayerLeft(p) => {
                    slot.remove(&p.index);
                }
                events::Event::PlayerJoined(p) => {
                    emitted.entry(p.player.name.clone()).or_default();
                    slot.insert(p.player.index, p.player.name);
                }
                events::Event::PlayerKilled(k) => {
                    let killer = k.killer.unwrap();
                    killers.insert((killer, k.tick), slot[&killer].clone());
                }
                events::Event::Medal {
                    player,
                    earned_at,
                    medal,
                } => {
                    let name = &killers[&(player, earned_at)];
                    *emitted.get_mut(name).unwrap().entry(medal).or_default() += 1;
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        emitted["Ada"],
        BTreeMap::from([("opening-act".into(), 1), ("twofer".into(), 1)])
    );
    assert_eq!(
        emitted["Ben"],
        BTreeMap::from([("payback".into(), 1), ("last-word".into(), 1)])
    );
    assert!(emitted["Cara"].is_empty());
    assert_eq!(summary, emitted);
    std::fs::remove_dir_all(dir).unwrap();
}
