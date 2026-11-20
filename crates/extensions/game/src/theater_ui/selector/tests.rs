use super::*;
use crate::theater_ui::adapter::{Clip, FilmMeta, Player, Source};

fn film(id: &str, map: &str, game_type: Option<&str>, started_ms: u64) -> Entry {
    Entry {
        film: FilmMeta {
            id: id.into(),
            map: map.into(),
            game_type: game_type.map(str::to_owned),
            started_ms,
            ..FilmMeta::default()
        },
        clip: None,
        archive_clip: None,
    }
}

fn ids(rows: &[Entry]) -> Vec<&str> {
    rows.iter().map(|row| row.film.id.as_str()).collect()
}

#[test]
fn sort_groups_by_map_or_game_type_with_newest_first_inside_groups() {
    let mut rows = vec![
        film("unknown", "", None, 300),
        film("older", "lockout", Some("Slayer"), 100),
        film("ctf", "zanzibar", Some("CTF"), 200),
        film("newer", "Lockout", Some("Slayer"), 200),
    ];
    let mut selector = Selector::default();
    selector.apply(&mut rows);
    assert_eq!(ids(&rows), ["unknown", "ctf", "newer", "older"]);
    selector.step_sort(true);
    selector.apply(&mut rows);
    assert_eq!(ids(&rows), ["newer", "older", "ctf", "unknown"]);
    selector.step_sort(true);
    selector.apply(&mut rows);
    assert_eq!(ids(&rows), ["ctf", "newer", "older", "unknown"]);
}

#[test]
fn map_and_game_type_filters_intersect_and_cycle_through_all() {
    let rows = vec![
        film("team", "maps/LOCKOUT.map", Some("Slayer"), 2),
        film("ctf", "lockout", Some("CTF"), 1),
        film("other", "zanzibar", Some("CTF"), 3),
    ];
    let mut selector = Selector::default();
    selector.step_filter(&rows, Filter::Map, true);
    assert_eq!(selector.map.as_deref(), Some("Lockout"));
    selector.step_filter(&rows, Filter::GameType, true);
    assert_eq!(selector.game_type.as_deref(), Some("CTF"));
    let mut filtered = rows.clone();
    selector.apply(&mut filtered);
    assert_eq!(ids(&filtered), ["ctf"]);
    selector.step_filter(&rows, Filter::Map, false);
    assert!(selector.map.is_none());
    let mut all_maps = rows;
    selector.apply(&mut all_maps);
    assert_eq!(ids(&all_maps), ["other", "ctf"]);
}

#[test]
fn a_clip_row_uses_match_metadata_and_its_own_duration() {
    let mut row = film("host", "maps/dune.map", Some("CTF"), 0);
    row.film.mode = Some("System Link".into());
    row.film.variant_name = "One Flag".into();
    row.film.duration_ticks = Some(18_000);
    row.film.source = Source::Host;
    row.film.scoreboard.push(Player {
        name: "Chief".into(),
        score: Some(4),
        ..Player::default()
    });
    row.clip = Some(Clip {
        film: "host".into(),
        from_tick: 300,
        to_tick: 1650,
    });
    assert_eq!(row.row_title(), "Relic / CTF");
    assert_eq!(
        row.browse_description(Some("CHIEF")),
        "System Link\nOne Flag\n01 Jan 70   0:45\n1 player   4 pts\nHost film"
    );
}

#[test]
fn live_host_rows_use_the_sealed_summary_and_recorded_rate() {
    let remote = serde_json::from_value::<live_hle::films::RecentFilm>(serde_json::json!({
        "game_id":"canonical", "started_at":"2026-11-20T00:00:00Z",
        "mode":"multiplayer", "map":"old", "ticks_per_second":60, "start_tick":400,
        "duration_s":90, "has_host_film":true,
        "players":[{"index":2, "name":"Chief", "team":0}],
        "scoreboard":[{"index":2, "score":40, "kills":12, "deaths":2, "assists":1}],
        "summary":{"level":"lockout","game_type":"ctf","mode":"system_link",
            "variant_name":"One Flag","duration_ticks":1800,"players":[
                {"seat":2,"name":"Chief","team":1,"score":3,"medals":{"double_kill":2}}]}
    }))
    .unwrap();
    let row = Entry {
        film: FilmMeta::try_from(remote).unwrap(),
        clip: None,
        archive_clip: None,
    };
    assert_eq!(row.film.id, "live:canonical");
    assert_eq!(row.film.live_start_tick, Some(400));
    assert_eq!(row.film.source, Source::Host);
    assert!(row.film.watch_unavailable().is_none());
    assert_eq!(row.film.mode.as_deref(), Some("System Link"));
    assert_eq!(row.row_title(), "Lockout / CTF");
    assert_eq!(row.duration(), "0:30");
    let p = &row.film.scoreboard[0];
    assert_eq!(
        (p.seat, p.score, p.kills, p.deaths),
        (Some(2), Some(3), Some(12), Some(2))
    );
    assert_eq!(p.medals["double_kill"], 2);
}
