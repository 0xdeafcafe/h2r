use super::*;
use crate::theater_ui::{
    Entry,
    adapter::{FilmMeta, Source},
    preview,
};
use crate::ui_kit::{
    layout::{Rules, check_bounds},
    preview::From,
};

fn track() -> Track {
    Track::from_entry(&Entry {
        film: FilmMeta {
            id: "synthetic".into(),
            source: Source::Host,
            compatibility: CompatibilityState::Playable,
            length_ticks: 900,
            roster: vec![Player {
                name: "Chief".into(),
                seat: Some(0),
                ..Default::default()
            }],
            ..Default::default()
        },
        clip: None,
        archive_clip: None,
    })
}

#[test]
fn every_pause_row_has_its_action_or_opens_the_kit_picker() {
    let mut track = track();
    track.tick = 450;
    track.marks = (Some(100), Some(300));
    let expected = [
        ("Resume", Some(Command::Resume), None),
        ("Restart Film", Some(Command::Restart), None),
        (
            "Change Player",
            None,
            Some((&["Chief"][..], Command::Player(0))),
        ),
        (
            "Camera",
            None,
            Some((
                &["Fly", "Orbit", "Follow", "First person"][..],
                Command::Camera(CameraMode::Free),
            )),
        ),
        ("Take Screenshot", Some(Command::Screenshot), None),
        (
            "Mark Clip",
            None,
            Some((&["Set In", "Set Out", "Clear Marks"][..], Command::MarkIn)),
        ),
        (
            "Export Clip",
            None,
            Some((
                &[
                    "PNG / 30 fps",
                    "PNG / 60 fps",
                    "MP4 / 30 fps",
                    "MP4 / 60 fps",
                ][..],
                Command::Export("png", 30),
            )),
        ),
        ("Save Film", Some(Command::Save), None),
        (
            "Share",
            None,
            Some((
                &["Private link", "Public film"][..],
                Command::Share("unlisted"),
            )),
        ),
        (
            "End Film",
            None,
            Some((&["Cancel", "End Film"][..], Command::Cancel)),
        ),
    ];
    let mut menu = Menu::default();
    for (index, (label, action, choices)) in expected.into_iter().enumerate() {
        let spec = menu.spec(&track, None);
        assert_eq!(spec.focus, index);
        let row = &spec.rows[index];
        assert_eq!(row.label, label);
        assert!(!row.cells(ValueStyle::Inline).0.contains('<'));
        assert!(!row.cells(ValueStyle::Inline).0.contains('>'));
        assert_eq!(menu.press(&track, Input::A), action);
        if let Some((choices, first)) = choices {
            let modal = menu.spec(&track, None);
            assert_eq!(modal.name, picker::GAME_NAME);
            assert_eq!(
                modal.dialog.as_ref().unwrap().base,
                crate::ui_kit::dialog::DialogBase::Large
            );
            assert_eq!(modal.underlay.as_ref().unwrap().focus, index);
            assert_eq!(
                modal
                    .rows
                    .iter()
                    .map(|r| r.label.as_str())
                    .collect::<Vec<_>>(),
                choices
            );
            assert_eq!(menu.press(&track, Input::B), None);
            assert_eq!(menu.spec(&track, None).focus, index);
            menu.press(&track, Input::A);
            assert_eq!(menu.press(&track, Input::A), Some(first));
        }
        menu.press(&track, Input::Down);
    }
    menu.press(&track, Input::A);
    menu.press(&track, Input::Down);
    assert_eq!(menu.press(&track, Input::A), Some(Command::End));
}

#[test]
fn absent_scoreboard_and_marks_have_distinct_disabled_rows() {
    let mut track = track();
    let mut menu = Menu::default();
    let spec = menu.spec(&track, None);
    assert!(
        spec.texts
            .iter()
            .any(|t| t.text == "No match stats available")
    );
    assert_eq!(spec.rows[2].press, Press::Open(picker::GAME_NAME.into()));
    assert_eq!(spec.rows[6].press, Press::Nothing);
    track.players.clear();
    assert_eq!(menu.spec(&track, None).rows[2].press, Press::Nothing);
    menu.press(&track, Input::Down);
    menu.press(&track, Input::Down);
    assert_eq!(menu.press(&track, Input::A), None);
    assert!(menu.spec(&track, None).dialog.is_none());
}

#[test]
fn timeline_marks_only_stats_kills_sidecar_and_ring_and_fills_toward_the_target() {
    let mut track = track();
    track.tick = 300;
    track.kills = vec![50, 50];
    track.keyframes = serde_json::from_value(serde_json::json!({
        "film": [100], "capture_points": [125], "ring": [175, 250], "sidecar": [500]
    }))
    .unwrap();
    track.marks = (Some(65), Some(95));
    track.seeking = true;
    track.jump = Some(Jump {
        target: 600,
        from_tick: 50,
        remaining_ticks: 300,
    });
    let spec = bar(&track, true, None);
    assert!(spec.texts.iter().any(|t| t.text.contains("Target 0:20")));
    assert!(spec.texts.iter().any(|t| t.text.contains("300 ticks left")));
    let art_count = |rgba| {
        spec.panel
            .iter()
            .filter(|a| a.art == Art::Solid { rgba })
            .count()
    };
    assert_eq!(art_count([230, 117, 101, 255]), 1);
    assert_eq!(art_count([146, 188, 251, 255]), 1);
    assert_eq!(art_count([132, 219, 190, 255]), 2);
    assert_eq!(art_count([248, 207, 102, 255]), 2);
    let progress = spec
        .panel
        .iter()
        .find(|a| {
            a.art
                == Art::Solid {
                    rgba: [136, 106, 59, 255],
                }
        })
        .unwrap();
    assert_eq!(progress.size[0], 141);
}

#[test]
fn playback_and_pause_preview_states_fit_without_exceptions() {
    let source = From::Synthetic;
    for name in [OVERLAY, PAUSE_MENU] {
        let live = preview::live(name).unwrap();
        for (state, spec) in live.specs() {
            let view = source.view(&spec, &state).unwrap();
            let rules = Rules::for_spec(&spec);
            let issues = check_bounds(&view, source.source(), &rules);
            assert!(issues.is_empty(), "{name}/{state}: {issues:?}");
            if let Some(parent) = view.underlay.as_deref() {
                let parent_spec = spec.underlay.as_deref().unwrap();
                let issues = check_bounds(parent, source.source(), &Rules::for_spec(parent_spec));
                assert!(issues.is_empty(), "{name}/{state} underlay: {issues:?}");
            }
        }
    }
}

#[test]
fn playback_states_render_distinct_transport_copy() {
    let mut page = preview::live(OVERLAY).unwrap();
    for (state, copy) in [
        ("playing", "Playing"),
        ("paused", "Paused"),
        ("opening", "Opening film"),
        ("jumping", "Jumping"),
        ("divergence", "Replay diverged"),
        ("unavailable", "Unavailable"),
    ] {
        assert!(page.go(state));
        let spec = page.spec();
        assert!(
            spec.texts.iter().any(|t| t.text.starts_with(copy)),
            "{state}"
        );
    }
    assert!(
        page.spec()
            .texts
            .iter()
            .any(|t| t.text.contains("breaking simulation change"))
    );
    assert!(page.go("hidden"));
    assert!(page.spec().texts.is_empty());
    assert!(page.go("preparing"));
    assert_eq!(page.spec().name, crate::theater_ui::DETAIL);
    assert_eq!(page.spec().help.as_deref(), Some("Preparing 2:40 / 8:26"));
}
