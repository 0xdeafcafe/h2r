//! Areas on netgame flags in the editor (docs/game/gametype-markers.md,
//! "Studio"), on the invented cache: the op, its undo and merging, the
//! refusals, an area following its flag through a swap-removal, and
//! converting a retail hill's corners.
use game_markers::{Area, Shape};

use crate::Error;
use crate::MapSource;
use crate::edit::{Command, History};
use crate::fixture::fixture;
use crate::layout::BlockKind;
use crate::scenario::{Scenario, Value};

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn scenario() -> Scenario {
    Scenario::read(&mut map::Map::from_reader(std::io::Cursor::new(fixture())).unwrap()).unwrap()
}

fn session(name: &str) -> crate::Session {
    let dir = std::env::temp_dir().join(format!("scenario-edit-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.map");
    std::fs::write(&path, fixture()).unwrap();
    let mut s = crate::Session::new();
    s.open(MapSource::file(&path)).unwrap();
    s
}

#[test]
fn an_area_op_sets_clears_and_undoes_and_a_drag_is_one_step() {
    let mut s = scenario();
    let mut h = History::default();
    assert_eq!(s.area(0), None);
    h.apply(&mut s, Command::area(0, Some(Area::radius(2.0))).merging(9))
        .unwrap();
    h.apply(&mut s, Command::area(0, Some(Area::radius(3.0))).merging(9))
        .unwrap();
    h.apply(&mut s, Command::area(0, Some(Area::radius(4.0))).merging(9))
        .unwrap();
    assert_eq!(s.area(0).unwrap().shape, Shape::Radius(4.0));
    h.seal();
    h.apply(&mut s, Command::area(0, None)).unwrap();
    assert_eq!(s.area(0), None);
    h.undo(&mut s).unwrap();
    assert_eq!(s.area(0).unwrap().shape, Shape::Radius(4.0));
    // The three drag steps were one.
    h.undo(&mut s).unwrap();
    assert_eq!(s.area(0), None);
    assert!(!h.can_undo());
    h.redo(&mut s).unwrap();
    assert_eq!(s.area(0).unwrap().shape, Shape::Radius(4.0));
}

#[test]
fn an_area_the_tools_may_not_store_is_refused_with_its_sentence() {
    let mut s = scenario();
    let mut h = History::default();
    let e = h
        .apply(&mut s, Command::area(0, Some(Area::radius(500.0))))
        .unwrap_err();
    let Error::Declined(why) = e else { panic!("{e}") };
    assert!(why.contains("radius"), "{why}");
    assert_eq!(s.area(0), None);
    assert!(!h.can_undo());
    // No such flag.
    assert!(matches!(
        h.apply(&mut s, Command::area(9, Some(Area::radius(2.0)))),
        Err(Error::NoEntry(..))
    ));
}

/// The area belongs to its flag, not to its slot: a swap-removal moves it
/// with the flag, and undoing the removal brings it back.
#[test]
fn an_area_follows_its_flag_through_a_swap_removal() {
    let mut s = scenario();
    let mut h = History::default();
    let like = s.new_entry(FLAGS, Some(0), Some([8.0, 0.0, 0.0])).unwrap();
    h.apply(&mut s, Command::add(FLAGS, like)).unwrap();
    h.apply(&mut s, Command::area(1, Some(Area::radius(3.0)))).unwrap();
    h.apply(&mut s, Command::area(0, Some(Area::radius(2.0)))).unwrap();
    // Removing flag 0 moves flag 1 (and its area) into slot 0.
    h.apply(&mut s, Command::remove(FLAGS, 0)).unwrap();
    assert_eq!(s.block(FLAGS).len(), 1);
    assert_eq!(s.area(0).unwrap().shape, Shape::Radius(3.0));
    h.undo(&mut s).unwrap();
    assert_eq!(s.area(0).unwrap().shape, Shape::Radius(2.0));
    assert_eq!(s.area(1).unwrap().shape, Shape::Radius(3.0));
}

/// Three flags of one hill type and identifier are a hill's corners and
/// take no area; a lone flag of the type does.
#[test]
fn a_hill_with_corners_takes_no_area() {
    let mut s = scenario();
    let mut h = History::default();
    for (i, at) in [[0.0, 0.0, 0.0], [6.0, 0.0, 0.0], [6.0, 6.0, 0.0]].iter().enumerate() {
        let mut flag = if i == 0 {
            Command::set(FLAGS, 0, "position", Value::Vec3(*at))
        } else {
            let e = s.new_entry(FLAGS, Some(0), Some(*at)).unwrap();
            Command::add(FLAGS, e)
        };
        flag.merge = None;
        h.apply(&mut s, flag).unwrap();
    }
    for i in 0..3 {
        h.apply(&mut s, Command::set(FLAGS, i, "type", Value::Int(11))).unwrap();
        h.apply(&mut s, Command::set(FLAGS, i, "identifier", Value::Int(0))).unwrap();
    }
    assert!(s.is_hill_corner(0));
    let e = h
        .apply(&mut s, Command::area(1, Some(Area::radius(2.0))))
        .unwrap_err();
    let Error::Declined(why) = e else { panic!("{e}") };
    assert_eq!(why, "a hill with corners takes no area");
    // Another hill number is a lone flag.
    h.apply(&mut s, Command::set(FLAGS, 2, "identifier", Value::Int(1))).unwrap();
    assert!(!s.is_hill_corner(2) && !s.is_hill_corner(0));
    h.apply(&mut s, Command::area(2, Some(Area::radius(2.0)))).unwrap();
}

/// Converting a retail hill leaves one flag at the hull's centre whose
/// zone holds the same points the corners' hull did, in one undo step.
#[test]
fn converting_a_hills_corners_gives_a_zone_that_holds_the_same_points() {
    let mut s = session("convert");
    let corners = [[0.0, 0.0, 1.0], [6.0, 0.0, 1.0], [6.0, 6.0, 1.5], [0.0, 6.0, 1.0]];
    for (i, at) in corners.iter().enumerate() {
        if i > 0 {
            let e = s.scenario().unwrap().new_entry(FLAGS, Some(0), None).unwrap();
            s.apply(Command::add(FLAGS, e)).unwrap();
        }
        s.apply(Command::set(FLAGS, i, "position", Value::Vec3(*at))).unwrap();
        s.apply(Command::set(FLAGS, i, "type", Value::Int(11))).unwrap();
        s.apply(Command::set(FLAGS, i, "identifier", Value::Int(0))).unwrap();
    }
    let before = Area::of_corners(&corners).unwrap();
    assert_eq!(s.hill_corners(2), vec![0, 1, 2, 3]);
    assert!(s.editable_area(2).is_none(), "a corner has no area to edit");

    let kept = s.convert_hill(2).unwrap();
    assert_eq!(s.scenario().unwrap().block(FLAGS).len(), 1);
    let (at, area, from) = s.editable_area(kept).unwrap();
    assert_eq!(from, crate::AreaFrom::Stored);
    assert_eq!(at, before.0);
    for p in [[3.0, 3.0, 1.2], [0.0, 0.0, 0.5], [6.0, 6.0, 3.0], [7.0, 3.0, 1.0], [3.0, 3.0, 3.1]] {
        assert_eq!(area.contains(at, p), before.1.contains(before.0, p), "{p:?}");
    }
    // One undo step brings the corners back.
    assert!(s.undo());
    assert_eq!(s.scenario().unwrap().block(FLAGS).len(), 4);
    assert_eq!(s.hill_corners(0).len(), 4);
    assert!(s.convert_hill(9).is_err());
}

#[test]
fn a_lone_hill_flag_edits_the_default_area_and_a_bomb_spawn_has_none() {
    let mut s = session("lone");
    // The fixture's flag is a bomb spawn: no area to shape.
    assert!(s.editable_area(0).is_none());
    s.apply(Command::set(FLAGS, 0, "type", Value::Int(11))).unwrap();
    s.apply(Command::set(FLAGS, 0, "identifier", Value::Int(0))).unwrap();
    let (_, area, from) = s.editable_area(0).unwrap();
    assert_eq!((from, area), (crate::AreaFrom::Default, Area::default()));
    s.set_area(0, Some(Area::radius(5.0)), None).unwrap();
    assert_eq!(s.editable_area(0).unwrap().2, crate::AreaFrom::Stored);
}
