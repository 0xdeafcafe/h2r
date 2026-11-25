//! Ground labels on netgame flags (docs/game/game-types-without-carriers.md,
//! "In Studio"), on the invented cache: the op and its undo, a label
//! following its flag through a swap-removal, placing one as one step, and
//! the collision stamp.
use crate::edit::{Command, History};
use crate::fixture::fixture;
use crate::layout::BlockKind;
use crate::scenario::{Scenario, Value};
use crate::{Error, FlagLabel, LabelKind, MapSource};

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn scenario() -> Scenario {
    Scenario::read(&mut map::Map::from_reader(std::io::Cursor::new(fixture())).unwrap()).unwrap()
}

fn session(name: &str) -> crate::Session {
    let dir = std::env::temp_dir().join(format!(
        "scenario-edit-ground-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.map");
    std::fs::write(&path, fixture()).unwrap();
    let mut s = crate::Session::new();
    s.set_allow_growth(true);
    s.open(MapSource::file(&path)).unwrap();
    s
}

fn drop_label() -> FlagLabel {
    FlagLabel {
        kind: LabelKind::Drop,
        phase: 0,
    }
}

#[test]
fn a_label_op_sets_clears_and_undoes() {
    let mut s = scenario();
    let mut h = History::default();
    assert_eq!(s.flag_label(0), None);
    h.apply(&mut s, Command::label(0, Some(drop_label())))
        .unwrap();
    assert_eq!(s.flag_label(0), Some(drop_label()));
    assert_eq!(s.flag_labels(), vec![(0, drop_label())]);
    h.apply(&mut s, Command::label(0, None)).unwrap();
    assert_eq!(s.flag_label(0), None);
    h.undo(&mut s).unwrap();
    assert_eq!(s.flag_label(0), Some(drop_label()));
    h.undo(&mut s).unwrap();
    assert_eq!(s.flag_label(0), None);
    // No such flag; a phase on a kind that takes none.
    assert!(matches!(
        h.apply(&mut s, Command::label(99, Some(drop_label()))),
        Err(Error::NoEntry(..))
    ));
    let phased = FlagLabel {
        kind: LabelKind::Drop,
        phase: 2,
    };
    assert!(matches!(
        h.apply(&mut s, Command::label(0, Some(phased))),
        Err(Error::Declined(_))
    ));
    assert_eq!(s.flag_label(0), None);
}

#[test]
fn a_label_follows_its_flag_through_a_swap_removal() {
    let mut s = scenario();
    let mut h = History::default();
    let like = s.new_entry(FLAGS, Some(0), Some([8.0, 0.0, 0.0])).unwrap();
    h.apply(&mut s, Command::add(FLAGS, like)).unwrap();
    let base = FlagLabel {
        kind: LabelKind::Base,
        phase: 0,
    };
    h.apply(&mut s, Command::label(1, Some(base))).unwrap();
    h.apply(&mut s, Command::label(0, Some(drop_label())))
        .unwrap();
    h.apply(&mut s, Command::remove(FLAGS, 0)).unwrap();
    assert_eq!(s.flag_labels(), vec![(0, base)]);
    h.undo(&mut s).unwrap();
    assert_eq!(s.flag_label(0), Some(drop_label()));
    assert_eq!(s.flag_label(1), Some(base));
}

/// The window's placing and `scenario.add` both come to `add_label_at`:
/// one step, a flag with the label, its facing and its team.
#[test]
fn placing_a_label_is_one_step_with_the_flags_own_fields() {
    let mut s = session("place");
    let n = s.scenario().unwrap().block(FLAGS).len();
    let at = [4.0, 5.0, 1.0];
    let index = s.add_label_at(LabelKind::Line, at, 1.5, 1, 3).unwrap();
    assert_eq!(index, n);
    let scenario = s.scenario().unwrap();
    assert_eq!(
        scenario.flag_label(index),
        Some(FlagLabel {
            kind: LabelKind::Line,
            phase: 3
        })
    );
    assert_eq!(
        scenario.get(FLAGS, index, "position").unwrap(),
        Value::Vec3(at)
    );
    assert_eq!(
        scenario.get(FLAGS, index, "facing").unwrap(),
        Value::Float(1.5)
    );
    assert_eq!(scenario.get(FLAGS, index, "team").unwrap(), Value::Int(1));
    assert_eq!(s.selected(), Some((FLAGS, index)));
    // A drop takes no team or phase: they are kept at 0.
    let d = s.add_label_at(LabelKind::Drop, at, 0.0, 1, 7).unwrap();
    let scenario = s.scenario().unwrap();
    assert_eq!(scenario.flag_label(d), Some(drop_label()));
    assert_eq!(scenario.get(FLAGS, d, "team").unwrap(), Value::Int(0));
    // Undo takes the flag and its label away in one step.
    assert!(s.undo());
    assert_eq!(s.scenario().unwrap().block(FLAGS).len(), n + 1);
    assert_eq!(s.scenario().unwrap().flag_label(d), None);
    assert!(s.undo());
    assert_eq!(s.scenario().unwrap().block(FLAGS).len(), n);
    assert!(s.scenario().unwrap().flag_labels().is_empty());
    // A team that is not red or blue is refused, with nothing placed.
    assert!(matches!(
        s.add_label_at(LabelKind::Base, at, 0.0, 2, 0),
        Err(Error::Declined(_))
    ));
    assert_eq!(s.scenario().unwrap().block(FLAGS).len(), n);
}

/// The stamp moves with what is solid and holds still for the rest.
#[test]
fn the_collision_stamp_moves_with_solids_and_not_with_flags() {
    let mut s = scenario();
    let mut h = History::default();
    let before = s.collision_stamp();
    assert_eq!(s.collision_stamp(), before);
    // A flag, a label and a move of a flag touch no collision.
    let like = s.new_entry(FLAGS, Some(0), Some([8.0, 0.0, 0.0])).unwrap();
    h.apply(&mut s, Command::add(FLAGS, like)).unwrap();
    h.apply(&mut s, Command::label(1, Some(drop_label())))
        .unwrap();
    h.apply(
        &mut s,
        Command::set(FLAGS, 1, "position", Value::Vec3([9.0, 1.0, 0.0])),
    )
    .unwrap();
    assert_eq!(s.collision_stamp(), before);
    // A solid object placed, moved and removed does.
    let scenery = BlockKind::Scenery;
    assert!(!s.block(scenery).is_empty(), "the fixture places a scenery");
    let at = s.get(scenery, 0, "position").unwrap();
    let Value::Vec3(p) = at else { panic!() };
    h.apply(
        &mut s,
        Command::set(
            scenery,
            0,
            "position",
            Value::Vec3([p[0] + 1.0, p[1], p[2]]),
        ),
    )
    .unwrap();
    assert_ne!(s.collision_stamp(), before);
    h.undo(&mut s).unwrap();
    assert_eq!(s.collision_stamp(), before);
}
