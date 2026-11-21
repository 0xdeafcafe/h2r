use super::*;

#[test]
fn a_live_summary_without_a_recording_still_shows_match_details() {
    let theater = Theater::default();
    let film = serde_json::from_value::<live_hle::films::RecentFilm>(serde_json::json!({
        "game_id": "played-game", "map": "lockout", "mode": "multiplayer",
        "kind": "matchmaking", "playlist": "Team Slayer",
        "variant": "Slayer", "started_at": "2026-11-20T02:26:06.627Z",
        "duration_s": 69, "result": "win", "has_host_film": false,
        "players": [{"index": 0, "name": "Chief", "team": 0}],
        "scoreboard": [{"index": 0, "score": 25, "kills": 25, "deaths": 8, "assists": 3}]
    }))
    .unwrap();
    let film = FilmMeta::try_from(film).unwrap();
    let mut state = theater.state();
    state.recent_archives.insert(film.id.clone());
    state.load = Load::Ready(vec![film]);
    drop(state);

    let recent = pages::spec(&theater, true);
    assert_eq!(recent.rows.len(), BROWSE_ROWS + 1);
    let row = &recent.rows[BROWSE_ROWS];
    assert_eq!(
        row.description.as_deref(),
        Some("20 Nov 26 / 1:09\nMatchmaking / Team Slayer\nSlayer\n1 player / Win\nLive summary")
    );

    let entry = theater.state().entries().pop().unwrap();
    theater.state().select(entry);
    let detail = pages::spec(&theater, false);
    assert_eq!(detail.title, "Lockout");
    assert_eq!(detail.rows.len(), 1);
    assert_eq!(detail.rows[0].label, "Save");
    assert_eq!(detail.help.as_deref(), Some("No host film was recorded."));
    let header: Vec<_> = detail
        .texts
        .iter()
        .take(4)
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(
        header,
        [
            "Slayer",
            "Matchmaking / Team Slayer",
            "20 Nov 26 / 1:09",
            "Result: Win"
        ]
    );
    for text in ["Chief", "Red", "25", "25 / 8"] {
        assert!(detail.texts.iter().any(|t| t.text == text));
    }
}

#[test]
fn sparse_summary_keeps_the_known_roster_and_unmatched_scores() {
    let film = serde_json::from_value::<live_hle::films::RecentFilm>(serde_json::json!({
        "game_id": "sparse", "map": "lockout", "mode": "multiplayer",
        "started_at": "2026-11-20T00:00:00Z",
        "players": [{"index": 0, "name": "Echo", "team": 0}],
        "scoreboard": [{"index": 1, "score": 8, "kills": 7, "deaths": 3, "assists": 0}],
        "summary": {"level":"lockout", "game_type":"slayer", "mode":"custom",
            "variant_name":"Slayer", "duration_ticks":300, "players":[]}
    }))
    .unwrap();
    let theater = Theater::default();
    theater.state().select(Entry {
        film: FilmMeta::try_from(film).unwrap(),
        clip: None,
        archive_clip: None,
    });
    let details = pages::spec(&theater, false);
    let roster: Vec<_> = details
        .texts
        .iter()
        .skip(5)
        .filter(|t| !t.text.is_empty())
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(
        roster,
        [
            "Players", "Team", "Score", "K / D", "Echo", "Red", "Player 2", "8", "7 / 3"
        ]
    );
}

#[test]
fn browser_tabs_list_picture_details_and_footer_have_room() {
    use crate::ui_kit::layout::{Rules, check_bounds};
    use crate::ui_kit::preview::From;

    let source = From::Synthetic;
    let mut library = live(FILMS).unwrap();
    let spec = library.spec();
    let view = source.view(&spec, "list").unwrap();
    for (i, label) in ["Sort: Newest", "Map: All", "Game type: All", "Lockout / Slayer"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(view.widget(&format!("row.{i}.label")).unwrap().text(), Some(label));
        assert!(view.widget(&format!("row.{i}.value")).is_none());
    }
    assert!(spec.panel.is_empty());
    assert!(spec.texts.iter().any(|t| t.text == "2 films"));

    for state in ["list", "full-library"] {
        assert!(library.go(state));
        let spec = library.spec();
        let view = source.view(&spec, state).unwrap();
        let tab_bottom = spec.texts[0].rect[2];
        let picture = view.widget("art.0").unwrap().rect;
        let detail = view.widget("description").unwrap().rect;
        let footer_top = view.widget("legend").unwrap().rect[0];
        assert!(tab_bottom > picture[0] && picture[2] > detail[0]);
        assert!(detail[2] > footer_top);
        for row in view.widgets.iter().filter(|w| w.id.starts_with("row.")) {
            assert!(row.rect[0] < tab_bottom && row.rect[2] > footer_top, "{}", row.id);
            assert!(row.rect[3] < detail[1], "{}", row.id);
        }
        let issues = check_bounds(&view, source.source(), &Rules::for_spec(&spec));
        assert!(issues.is_empty(), "{state}: {issues:?}");
    }
}

#[test]
fn empty_library_explains_the_next_action_on_every_control() {
    let mut sample = live(FILMS).unwrap();
    for (state, help) in [
        ("empty", "No recent films\nPlay a game to record a film."),
        (
            "empty-saved",
            "No saved films\nOpen a film in Recent and choose Save.",
        ),
        (
            "empty-clips",
            "No clips\nWhile watching a film, press X to mark the start and end of a clip.",
        ),
        (
            "no-matches",
            "No matching films\nChoose All in Map or Game type.",
        ),
        ("loading", "Reading films..."),
    ] {
        assert!(sample.go(state));
        for i in 0..BROWSE_ROWS {
            let spec = sample.spec();
            assert_eq!(spec.rows.len(), BROWSE_ROWS);
            assert_eq!(spec.focus, i);
            assert!(spec.texts.iter().any(|t| t.text == help));
            assert_eq!(
                spec.description_text().as_deref(),
                Some(super::super::selector::OptionRow::ALL[i].description()),
            );
            sample.press(Input::Down);
        }
    }
}

#[test]
fn theater_tabs_only_show_matching_entries() {
    let mut view = live(FILMS).unwrap();
    assert!(view.go("saved"));
    assert_eq!(view.spec().rows.len(), BROWSE_ROWS + 1);
    assert!(view.spec().rows[BROWSE_ROWS].label.contains("Lockout"));
    assert!(view.go("clips"));
    assert_eq!(
        view.spec().rows[BROWSE_ROWS].description.as_deref().and_then(|s| s.lines().next()),
        Some("19 Nov 26 / 0:40")
    );
}

#[test]
fn theater_tab_walkthrough_restores_the_parent_focus() {
    let mut view = live(FILMS).unwrap();
    // Left and Right switch libraries even with a film selected.
    assert_eq!(view.spec().focus, BROWSE_ROWS);
    view.press(Input::Right);
    assert_eq!(view.spec().rows.len(), BROWSE_ROWS + 1);
    assert_eq!(
        view.spec()
            .texts
            .iter()
            .filter(|t| !t.text.is_empty())
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>(),
        ["Recent", "SAVED", "Clips", "1 film"]
    );
    view.press(Input::A);
    assert_eq!(view.spec().name, DETAIL);
    view.press(Input::B);
    assert_eq!(view.spec().name, FILMS);
    assert_eq!(view.spec().focus, BROWSE_ROWS);
}

#[test]
fn theater_unplayable_film_can_be_saved_but_cannot_watch() {
    let mut view = live(DETAIL).unwrap();
    assert!(view.go("unplayable"));
    assert_eq!(
        view.spec().help.as_deref(),
        Some("Needs a different build.")
    );
    assert_eq!(view.spec().rows[0].label, "Save");
    view.press(Input::A);
    assert_eq!(view.spec().name, DETAIL);
    assert_eq!(view.spec().rows[0].label, "Unsave");
}

#[test]
fn summary_without_players_has_one_roster_message_and_one_action() {
    let mut view = live(DETAIL).unwrap();
    assert!(view.go("summary"));
    let spec = view.spec();
    let labels: Vec<_> = spec.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Save"]);
    let roster: Vec<_> = spec
        .texts
        .iter()
        .skip(5)
        .filter(|t| !t.text.is_empty())
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(roster, ["Players", "No players were recorded."]);
    assert_eq!(spec.help.as_deref(), Some("No host film was recorded."));
}

#[test]
fn legacy_clip_only_offers_delete_and_explains_playback_once() {
    let theater = Theater::default();
    let mut film = fixture("legacy", "lockout", "Slayer", true);
    film.source = Source::Local;
    film.version = Some(1);
    let clip = Clip {
        film: film.id.clone(),
        from_tick: 0,
        to_tick: 120,
    };
    theater.state().select(Entry {
        film,
        clip: Some(clip),
        archive_clip: None,
    });
    let detail = pages::spec(&theater, false);
    assert_eq!(detail.rows.len(), 1);
    assert_eq!(detail.rows[0].label, "Delete");
    assert_eq!(detail.help.as_deref(), Some("No playback track was saved."));
    assert!(detail.texts.iter().any(|t| t.text.ends_with(" / 0:04")));
    theater.step_detail(0);
    assert_eq!(pages::spec(&theater, false).rows[0].label, "Confirm delete");
}

#[test]
fn roster_headings_and_values_share_column_edges_and_alignment() {
    let view = live(DETAIL).unwrap();
    let spec = view.spec();
    for (label, value, right) in [
        ("Players", "Chief", false),
        ("Team", "Red", false),
        ("Score", "21", true),
        ("K / D", "17 / 8", true),
    ] {
        let heading = spec.texts.iter().find(|t| t.text == label).unwrap();
        let cell = spec.texts.iter().find(|t| t.text == value).unwrap();
        assert_eq!(
            (heading.rect[1], heading.rect[3], heading.right),
            (cell.rect[1], cell.rect[3], right)
        );
        assert_eq!(cell.right, right);
        assert_eq!(
            heading.rect[0] - cell.rect[0],
            crate::mods_screen::page_layout::ROW_STEP
        );
    }
}

#[test]
fn names_only_roster_pages_without_empty_stat_columns() {
    let mut view = live(DETAIL).unwrap();
    assert!(view.go("players"));
    let roster = |spec: ScreenSpec| {
        spec.texts
            .into_iter()
            .skip(5)
            .map(|t| t.text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        roster(view.spec()),
        [
            "Players 1 / 2",
            "Player 1",
            "Player 2",
            "Player 3",
            "Player 4",
            "Player 5",
            "Player 6",
            "Player 7"
        ]
    );
    view.press(Input::Right);
    assert_eq!(
        roster(view.spec()),
        ["Players 2 / 2", "Player 8", "Player 9", "Player 10"]
    );
    view.press(Input::Left);
    assert_eq!(roster(view.spec())[0], "Players 1 / 2");
}

#[test]
fn theater_delete_is_confirmed_and_restores_the_library() {
    let mut view = live(DETAIL).unwrap();
    view.press(Input::Down);
    view.press(Input::Down);
    view.press(Input::A);
    assert_eq!(view.spec().rows[2].label, "Confirm delete");
    view.press(Input::A);
    assert_eq!(view.spec().rows.len(), BROWSE_ROWS + 1);
    assert_eq!(view.spec().name, FILMS);
}

#[test]
fn library_choices_open_a_modal_and_restore_the_option_on_choose_or_back() {
    let mut view = live(FILMS).unwrap();
    for _ in 0..BROWSE_ROWS {
        view.press(Input::Up);
    }
    for (row, title, labels) in [
        (0, "SORT", &["Newest", "Map", "Game type"][..]),
        (1, "MAP", &["All", "Lockout", "Zanzibar"][..]),
        (2, "GAME TYPE", &["All", "CTF", "Slayer"][..]),
    ] {
        let first = view.spec().rows[row].plain_value();
        view.press(Input::A);
        let modal = view.spec();
        assert_eq!(modal.name, picker::NAME);
        assert_eq!(modal.title, title);
        assert_eq!(
            modal
                .rows
                .iter()
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>(),
            labels
        );
        assert!(modal.dialog.is_some());
        let rendered = crate::ui_kit::preview::From::Synthetic
            .view(&modal, "open")
            .unwrap();
        let parent = rendered.underlay.as_deref().expect("visible film library");
        assert_eq!(parent.screen, FILMS);
        assert_eq!(parent.focus, Some(format!("row.{row}")));
        let library = modal.underlay.unwrap();
        assert_eq!(library.name, FILMS);
        assert_eq!(library.focus, row);
        view.press(Input::Down);
        view.press(Input::B);
        assert_eq!(view.spec().name, FILMS);
        assert_eq!(view.spec().focus, row);
        assert_eq!(view.spec().rows[row].plain_value(), first);
        view.press(Input::A);
        view.press(Input::Down);
        view.press(Input::A);
        assert_eq!(view.spec().name, FILMS);
        assert_eq!(view.spec().focus, row);
        assert_eq!(
            view.spec().rows[row].plain_value().as_deref(),
            Some(labels[1])
        );
        view.press(Input::A);
        assert_eq!(view.spec().focus, 1);
        view.press(Input::A);
        assert_eq!(
            view.spec().rows[row].plain_value().as_deref(),
            Some(labels[1])
        );
        view.press(Input::Down);
    }
}

#[test]
fn an_empty_library_still_opens_the_map_picker_on_all() {
    let mut view = live(FILMS).unwrap();
    view.go("empty");
    view.press(Input::Down);
    view.press(Input::A);
    assert_eq!(view.spec().title, "MAP");
    assert_eq!(view.spec().rows.len(), 1);
    assert_eq!(view.spec().rows[0].label, "All");
    let source = crate::ui_kit::preview::From::Synthetic;
    let open = source.view(&view.spec(), "open").unwrap();
    let parent = open.underlay.as_deref().unwrap();
    let draw = |screen: &crate::ui_kit::view::View, widgets| {
        crate::ui_kit::preview::render_view_where(screen, source.source(), &|_| widgets, false)
    };
    let library = draw(parent, true);
    assert!(
        draw(&open, false).rgba == library.rgba,
        "the library remains behind the modal"
    );
    assert!(
        draw(&open, true).rgba != library.rgba,
        "the picker is visible on top"
    );
    view.press(Input::A);
    assert_eq!(view.spec().name, FILMS);
    assert_eq!(view.spec().focus, 1);
}

#[test]
fn film_art_follows_the_focused_entry_and_controls_keep_the_frame_clear() {
    use crate::ui_kit::view::Art;

    let mut view = live(FILMS).unwrap();
    view.go("saved");
    let mut spec = view.spec();
    for i in 0..BROWSE_ROWS {
        spec.focus = i;
        assert!(
            spec.shown_art()
                .iter()
                .all(|a| !matches!(a.art, Art::Level { .. }))
        );
    }
    spec.focus = BROWSE_ROWS;
    let map_art = Art::Level {
        stem: "lockout".into(),
    };
    assert!(spec.shown_art().iter().any(|a| a.art == map_art));

    view.go("empty");
    let spec = view.spec();
    assert!(spec.shown_art().is_empty());
    assert!(spec.rows.iter().all(|row| row.art.is_none()));

    let detail = live(DETAIL).unwrap().spec();
    assert_eq!(detail.art.len(), 1);
    assert_eq!(detail.art[0].art, map_art);
}

#[test]
fn retail_corners_frame_the_detail_picture_only() {
    use crate::ui_kit::{dialog, view::Art};

    let screen = live(DETAIL).unwrap().spec();
    let picture = screen.art.iter().find(|a| matches!(a.art, Art::Level { .. })).unwrap();
    let [left, top] = picture.at;
    let [width, height] = picture.size.map(|n| n as i16);
    let side = dialog::FRAME_CORNER_SIZE as i16;
    assert_eq!(screen.panel.len(), 4);
    for ((corner, index), at) in screen.panel[2..]
        .iter()
        .zip(dialog::FRAME_CORNERS)
        .zip([[left, top], [left + width - side, top - height + side]])
    {
        assert_eq!(
            corner.art,
            Art::Screen {
                screen_id: dialog::LARGE_SCREEN_ID,
                pane: 0,
                index,
                image: 0,
            }
        );
        assert_eq!(corner.at, at);
        assert_eq!(corner.size, [dialog::FRAME_CORNER_SIZE; 2]);
    }
}

#[test]
fn theater_overlay_hides_and_restores_while_paused() {
    let mut view = live(OVERLAY).unwrap();
    assert!(view.go("playing"));
    view.press(Input::A);
    assert!(
        view.spec()
            .texts
            .iter()
            .any(|t| t.text.starts_with("Paused"))
    );
    view.press(Input::Back);
    assert!(view.spec().texts.is_empty());
    view.press(Input::Back);
    assert!(
        view.spec()
            .texts
            .iter()
            .any(|t| t.text.starts_with("Paused"))
    );
}
