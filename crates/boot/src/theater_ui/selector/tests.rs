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
