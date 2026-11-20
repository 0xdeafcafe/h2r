//! Multi-selection on the invented cache (`crate::fixture`): two weapons,
//! two starting locations, one flag, one netgame item, one scenery.
use crate::fixture::fixture;
use crate::geometry::Ray;
use crate::layout::BlockKind;
use crate::save::MapSource;
use crate::scenario::Value;
use crate::session::{FieldEdit, Session};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(tag: &str) -> (Temp, Session) {
    let dir =
        std::env::temp_dir().join(format!("scenario-edit-multi-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("source.map");
    std::fs::write(&path, fixture()).unwrap();
    let mut ed = Session::new();
    ed.build_scenes = false;
    ed.open(MapSource::file(&path)).unwrap();
    (Temp(dir), ed)
}

fn pos(ed: &Session, (kind, i): (BlockKind, usize)) -> [f32; 3] {
    ed.scenario()
        .unwrap()
        .get(kind, i, "position")
        .unwrap()
        .as_vec3()
        .unwrap()
}

const W0: (BlockKind, usize) = (BlockKind::Weapons, 0);
const W1: (BlockKind, usize) = (BlockKind::Weapons, 1);
const S0: (BlockKind, usize) = (BlockKind::StartingLocations, 0);

#[test]
fn toggling_builds_a_selection_and_selecting_one_drops_it() {
    let (_t, mut ed) = open("toggle");
    ed.select(Some(W0));
    ed.toggle_select(W1);
    assert_eq!(ed.selection(), [W1, W0], "the newest is the primary");
    assert_eq!(ed.selected(), Some(W1));
    ed.toggle_select(S0);
    assert_eq!(ed.selection(), [S0, W1, W0]);
    assert!(ed.is_selected(W0));
    // Toggling the primary off hands the gizmo to the next.
    ed.toggle_select(S0);
    assert_eq!(ed.selection(), [W1, W0]);
    ed.select(Some(S0));
    assert_eq!(ed.selection(), [S0]);
    assert!(!ed.is_selected(W0));
    ed.select(None);
    assert!(ed.selection().is_empty());
}

#[test]
fn shared_fields_are_those_every_selected_block_has() {
    let (_t, mut ed) = open("shared");
    ed.select_many(vec![W0, W1]);
    let names: Vec<&str> = ed.shared_fields().iter().map(|f| f.name).collect();
    assert!(names.contains(&"rotation") && names.contains(&"palette index"));
    ed.select_many(vec![W0, S0]);
    let names: Vec<&str> = ed.shared_fields().iter().map(|f| f.name).collect();
    // A weapon and a spawn share only their position (the spawn's team
    // and facing are its own; the weapon's rotation is a vector).
    assert_eq!(names, ["position"]);
}

#[test]
fn one_component_set_on_many_keeps_the_others_and_undoes_in_one_step() {
    let (_t, mut ed) = open("component");
    let before = [pos(&ed, W0), pos(&ed, W1), pos(&ed, S0)];
    ed.select_many(vec![W0, W1, S0]);
    let key = ed.new_key();
    let n = ed
        .edit_selection("position", FieldEdit::Component(2, 7.5), key)
        .unwrap();
    assert_eq!(n, 3);
    // A drag in the inspector: more edits with the same key merge.
    ed.edit_selection("position", FieldEdit::Component(2, 8.0), key)
        .unwrap();
    for (i, e) in [W0, W1, S0].into_iter().enumerate() {
        let p = pos(&ed, e);
        assert_eq!(p[2], 8.0);
        assert_eq!(p[..2], before[i][..2], "x and y stay each one's own");
    }
    assert!(ed.undo());
    assert_eq!([pos(&ed, W0), pos(&ed, W1), pos(&ed, S0)], before);
    assert!(!ed.can_undo(), "one undo step");
    assert!(ed.redo());
    assert_eq!(pos(&ed, W1)[2], 8.0);
}

#[test]
fn a_whole_value_skips_blocks_without_the_field() {
    let (_t, mut ed) = open("whole");
    ed.select_many(vec![W0, S0]);
    let n = ed
        .edit_selection("scale", FieldEdit::Set(Value::Float(2.0)), 1)
        .unwrap();
    assert_eq!(n, 1, "the spawn has no scale");
    let s = ed.scenario().unwrap();
    assert_eq!(s.get(W0.0, W0.1, "scale").unwrap(), Value::Float(2.0));
}

#[test]
fn deleting_many_is_one_step_and_undo_restores_every_entry() {
    let (_t, mut ed) = open("delete");
    let before = ed.scenario().unwrap().changes();
    let w1 = pos(&ed, W1);
    let s1 = pos(&ed, (S0.0, 1));
    ed.select_many(vec![W0, W1, S0]);
    assert_eq!(ed.remove_selection().unwrap(), 3);
    let s = ed.scenario().unwrap();
    assert_eq!(s.block(BlockKind::Weapons).len(), 0);
    assert_eq!(s.block(BlockKind::StartingLocations).len(), 1);
    assert_eq!(pos(&ed, (S0.0, 0)), s1, "the other spawn took the slot");
    assert!(ed.selection().is_empty());
    assert!(ed.undo());
    assert_eq!(ed.scenario().unwrap().changes(), before);
    assert_eq!(pos(&ed, W1), w1);
    assert!(!ed.can_undo());
}

#[test]
fn deleting_the_lower_index_first_would_move_the_higher_so_the_higher_goes_first() {
    let (_t, mut ed) = open("order");
    let w0 = pos(&ed, W0);
    let w1 = pos(&ed, W1);
    ed.set_allow_growth(true);
    // Three weapons; deleting 0 and 2 must leave the middle one.
    ed.select(Some(W1));
    ed.duplicate_selection().unwrap();
    let w2 = (BlockKind::Weapons, 2);
    ed.edit_selection("position", FieldEdit::Set(Value::Vec3([9.0; 3])), 2)
        .unwrap();
    ed.select_many(vec![W0, w2]);
    ed.remove_selection().unwrap();
    assert_eq!(ed.scenario().unwrap().block(BlockKind::Weapons).len(), 1);
    assert_eq!(pos(&ed, W0), w1);
    assert_ne!(pos(&ed, W0), w0);
}

#[test]
fn duplicating_many_copies_in_place_selects_the_copies_and_respects_room() {
    let (_t, mut ed) = open("duplicate");
    ed.select_many(vec![W0, W1]);
    // The fixture's blocks are full: growth is off.
    assert_eq!(
        ed.duplicate_selection_refusal(),
        Some(crate::GROWTH_REFUSAL)
    );
    assert!(ed.duplicate_selection().is_err());
    ed.set_allow_growth(true);
    let copies = ed.duplicate_selection().unwrap();
    assert_eq!(copies, [(BlockKind::Weapons, 2), (BlockKind::Weapons, 3)]);
    assert_eq!(ed.selection(), copies);
    assert_eq!(pos(&ed, (BlockKind::Weapons, 2)), pos(&ed, W0));
    assert_eq!(pos(&ed, (BlockKind::Weapons, 3)), pos(&ed, W1));
    assert!(ed.undo());
    assert_eq!(ed.scenario().unwrap().block(BlockKind::Weapons).len(), 2);
    assert!(ed.selection().is_empty(), "the copies are gone");
}

#[test]
fn a_gizmo_move_of_the_primary_carries_the_rest() {
    let (_t, mut ed) = open("gizmo");
    let (w1, s0) = (pos(&ed, W1), pos(&ed, S0));
    ed.select_many(vec![W0, W1, S0]);
    let t = ed.selected_transform().unwrap();
    let to = [t.position[0] + 1.0, t.position[1], t.position[2] - 2.0];
    ed.gesture_set(to, t.ypr()).unwrap();
    ed.end_gesture();
    assert_eq!(pos(&ed, W1), [w1[0] + 1.0, w1[1], w1[2] - 2.0]);
    assert_eq!(pos(&ed, S0), [s0[0] + 1.0, s0[1], s0[2] - 2.0]);
    // Turning turns each by the same yaw; the spawn's facing too.
    let facing = |ed: &Session| {
        ed.scenario()
            .unwrap()
            .get(S0.0, S0.1, "facing")
            .unwrap()
            .as_f32()
            .unwrap()
    };
    let f0 = facing(&ed);
    let t = ed.selected_transform().unwrap();
    let ypr = t.ypr();
    ed.gesture_set(t.position, [ypr[0] + 0.5, ypr[1], ypr[2]])
        .unwrap();
    ed.end_gesture();
    assert!((facing(&ed) - (f0 + 0.5)).abs() < 1e-6);
    // Two gestures, two undo steps, back to the start.
    assert!(ed.undo() && ed.undo());
    assert!(!ed.can_undo());
    assert_eq!(pos(&ed, W1), w1);
}

#[test]
fn scaling_scales_every_selected_entry_with_a_scale_by_the_same_factor() {
    let (_t, mut ed) = open("scale");
    ed.select(Some(W1));
    ed.edit_selection("scale", FieldEdit::Set(Value::Float(2.0)), 2)
        .unwrap();
    ed.select_many(vec![W0, W1, S0]);
    assert_eq!(ed.selected_scale(), Some(1.0));
    ed.gesture_scale(1.5).unwrap();
    ed.gesture_scale(3.0).unwrap();
    ed.end_gesture();
    let s = ed.scenario().unwrap();
    assert_eq!(s.get(W0.0, W0.1, "scale").unwrap(), Value::Float(3.0));
    assert_eq!(s.get(W1.0, W1.1, "scale").unwrap(), Value::Float(6.0));
    assert!(ed.undo());
    assert_eq!(
        ed.scenario().unwrap().get(W1.0, W1.1, "scale").unwrap(),
        Value::Float(2.0)
    );
}

#[test]
fn placing_at_the_cursor_puts_the_entry_where_the_ray_meets_the_level() {
    let (_t, mut ed) = open("place");
    ed.set_allow_growth(true);
    let ray = Ray {
        origin: [3.0, 4.0, 50.0],
        dir: [0.0, 0.0, -1.0],
    };
    // The invented map has no BSP: nothing under the cursor.
    assert_eq!(ed.surface_hit(&ray), None);
    let at = [3.0, 4.0, 0.5];
    let i = ed.add_entry_at(BlockKind::Weapons, Some(0), at).unwrap();
    assert_eq!(i, 2);
    assert_eq!(pos(&ed, (BlockKind::Weapons, 2)), at);
    assert_eq!(ed.selected(), Some((BlockKind::Weapons, 2)));
    assert!(ed.undo());
    assert_eq!(ed.scenario().unwrap().block(BlockKind::Weapons).len(), 2);
}

#[test]
fn the_selection_sphere_holds_every_selected_glyph() {
    let (_t, mut ed) = open("sphere");
    ed.select_many(vec![W0, W1]);
    let (c, r) = ed.selection_sphere().unwrap();
    for p in [pos(&ed, W0), pos(&ed, W1)] {
        let d = crate::geometry::sub(p, c);
        assert!(crate::geometry::dot(d, d).sqrt() <= r + 1e-4);
    }
    ed.select(None);
    assert_eq!(ed.selection_sphere(), None);
}

#[test]
fn one_flag_bit_set_on_many_keeps_each_entrys_other_bits() {
    let (_t, mut ed) = open("bit");
    let flags =
        |ed: &Session, i| match ed
            .scenario()
            .unwrap()
            .get(BlockKind::Weapons, i, "placement flags")
        {
            Ok(Value::Int(x)) => x,
            v => panic!("flags {v:?}"),
        };
    // Give the two weapons different bits first.
    ed.select(Some(W0));
    let key = ed.new_key();
    ed.edit_selection("placement flags", FieldEdit::Set(Value::Int(0b0001)), key)
        .unwrap();
    ed.seal();
    ed.select(Some(W1));
    let key = ed.new_key();
    ed.edit_selection("placement flags", FieldEdit::Set(Value::Int(0b0100)), key)
        .unwrap();
    ed.seal();

    ed.select_many(vec![W0, W1]);
    let key = ed.new_key();
    let n = ed
        .edit_selection("placement flags", FieldEdit::Bit(1, true), key)
        .unwrap();
    assert_eq!(n, 2);
    assert_eq!((flags(&ed, 0), flags(&ed, 1)), (0b0011, 0b0110));
    ed.seal();
    let key = ed.new_key();
    ed.edit_selection("placement flags", FieldEdit::Bit(0, false), key)
        .unwrap();
    assert_eq!((flags(&ed, 0), flags(&ed, 1)), (0b0010, 0b0110));
    ed.seal();
    // Each bit edit is one undo step over both weapons.
    ed.undo();
    assert_eq!((flags(&ed, 0), flags(&ed, 1)), (0b0011, 0b0110));
    ed.undo();
    assert_eq!((flags(&ed, 0), flags(&ed, 1)), (0b0001, 0b0100));
    // A bit means nothing on a vector.
    assert_eq!(FieldEdit::Bit(0, true).apply(Value::Vec3([0.0; 3])), None);
}
