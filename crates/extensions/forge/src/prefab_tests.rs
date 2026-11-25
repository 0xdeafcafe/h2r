//! Labels and prefabs over invented crates: a label is one op and goes
//! with its object, a prefab places as one undo step, a tag the map lacks
//! is refused by name, and the file reads back.
use super::{Fake, Files, frame, session};
use crate::{Endpoint, Frame, Label, OpId, Prefab, Session, TagKey, files};
use forge_doc::{BlockKind, Edit, Forge};

fn label(text: &str, number: Option<i32>) -> Label {
    Label::new(text, number).unwrap()
}

fn at(id: OpId) -> Endpoint {
    Endpoint::Added(id)
}

fn near(a: Frame, b: Frame) {
    for i in 0..3 {
        assert!(
            (a.position[i] - b.position[i]).abs() < 1e-3
                && (a.rotation[i] - b.rotation[i]).abs() < 1e-3,
            "{a:?} is not {b:?}"
        );
    }
}

/// A crate at each of x = 0, 2 and 4, the middle one labelled, as a build.
fn build() -> (Session, Fake, Vec<OpId>) {
    let mut s = session(12);
    let mut w = Fake::default();
    let ids: Vec<OpId> = [0., 2., 4.]
        .map(|x| s.place(&mut w, 1, 0, frame(x, 0.3)).unwrap())
        .to_vec();
    s.set_label(&mut w, 2, at(ids[1]), Some(label("checkpoint", Some(3))))
        .unwrap();
    (s, w, ids)
}

fn members(ids: &[OpId]) -> Vec<Endpoint> {
    ids.iter().copied().map(at).collect()
}

#[test]
fn a_label_is_one_op_that_replaces_and_clears() {
    let mut s = session(4);
    let mut w = Fake::default();
    let id = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    assert_eq!(s.label_of(at(id)), None);
    s.set_label(&mut w, 2, at(id), Some(label("hill", Some(0))))
        .unwrap();
    assert!(matches!(
        &s.document().ops.last().unwrap().edit,
        Edit::Forge(Forge::Label { label: Some(l), .. }) if l.text == "hill"
    ));
    assert_eq!(s.document().format, forge_doc::FORMAT_LABELS);
    s.set_label(&mut w, 3, at(id), Some(label("hill", Some(1))))
        .unwrap();
    assert_eq!(s.label_of(at(id)), Some(label("hill", Some(1))));
    // The same label again writes nothing.
    let ops = s.document().ops.len();
    s.set_label(&mut w, 4, at(id), Some(label("hill", Some(1))))
        .unwrap();
    assert_eq!(s.document().ops.len(), ops);
    s.set_label(&mut w, 5, at(id), None).unwrap();
    assert_eq!(s.label_of(at(id)), None);
    assert!(
        s.set_label(&mut w, 6, at(id), None)
            .unwrap_err()
            .contains("no label")
    );
}

#[test]
fn a_label_a_script_cannot_read_or_the_ground_owns_is_refused() {
    let mut s = session(4);
    let mut w = Fake::default();
    let id = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    let bad = Label {
        text: "Has Capitals".into(),
        number: None,
    };
    assert!(s.set_label(&mut w, 2, at(id), Some(bad)).is_err());
    let ground = forge_doc::GroundLabel::ALL[0].text();
    let error = s
        .set_label(&mut w, 3, at(id), Some(label(ground, None)))
        .unwrap_err();
    assert!(error.contains("AI and objectives"), "{error}");
}

#[test]
fn a_siege_barrier_is_picked_for_an_object_and_numbered_by_its_phase() {
    let mut s = session(4);
    let mut w = Fake::default();
    let id = s.place(&mut w, 1, 0, frame(1., 0.)).unwrap();
    s.label_barrier(&mut w, 2, at(id), 1).unwrap();
    assert_eq!(s.label_of(at(id)), Some(label("siege_barrier", Some(1))));
    let i = forge_doc::GroundLabel::ALL
        .iter()
        .position(|l| *l == forge_doc::GroundLabel::Barrier)
        .unwrap();
    assert_eq!(s.ground_counts()[i], 1, "the palette counts it");
    assert!(s.markers().unwrap().is_empty(), "it is no flag marker");
    // Picking it again renumbers; typing it in Properties still works.
    s.label_barrier(&mut w, 3, at(id), 2).unwrap();
    assert_eq!(s.label_of(at(id)), Some(label("siege_barrier", Some(2))));
    s.set_label(&mut w, 4, at(id), Some(label("siege_barrier", Some(0)))).unwrap();
    assert_eq!(s.label_of(at(id)), Some(label("siege_barrier", Some(0))));
    // It labels an object: the marker placement refuses it, by name.
    let error = s
        .place_marker(&mut w, 5, forge_doc::GroundLabel::Barrier, frame(0., 0.), 0, None)
        .unwrap_err();
    assert!(error.contains("labels an object"), "{error}");
    // A thing that is not there is refused.
    assert!(s.label_barrier(&mut w, 6, at(OpId(999)), 1).is_err());
}

#[test]
fn deleting_a_labelled_object_clears_its_label_in_the_same_undo_step() {
    let (mut s, mut w, ids) = build();
    s.delete(&mut w, 3, ids[1]).unwrap();
    assert!(s.thing(at(ids[1])).is_none());
    assert_eq!(s.label_of(at(ids[1])), None);
    s.undo(&mut w, 4).unwrap();
    assert!(s.thing(at(ids[1])).is_some());
    assert_eq!(s.label_of(at(ids[1])), Some(label("checkpoint", Some(3))));
}

#[test]
fn a_prefab_keeps_frames_from_the_pivot_and_its_labels() {
    let (s, _, ids) = build();
    let prefab = s.capture_prefab("wall", &members(&ids)).unwrap();
    assert!(prefab.grouped);
    assert_eq!(prefab.pieces.len(), 3);
    assert_eq!(prefab.pieces[1].label, Some(label("checkpoint", Some(3))));
    assert_eq!(prefab.pieces[0].position.map(|f| f.get()), [0., 0., 0.]);
    // The offsets are as if the pivot faced yaw 0: the second crate is 2
    // units from the first along world x, which the pivot's yaw of 0.3
    // turns to -0.3 from the pivot's own way.
    let [x, y, _] = prefab.pieces[1].position.map(|f| f.get());
    assert!((x - 2. * 0.3f32.cos()).abs() < 1e-3, "{x}");
    assert!((y + 2. * 0.3f32.sin()).abs() < 1e-3, "{y}");
    // The file form reads back to the same prefab.
    let json = prefab.canonical_json().unwrap();
    assert_eq!(Prefab::from_bytes(json.as_bytes()).unwrap(), prefab);
}

#[test]
fn placing_a_prefab_is_one_undo_step_and_turns_with_the_yaw_it_is_put_down_at() {
    let (mut s, mut w, ids) = build();
    let prefab = s.capture_prefab("wall", &members(&ids)).unwrap();
    let before = s.document().ops.len();
    let at_frame = Frame {
        position: [-10., -10., 2.],
        rotation: [1.0, 0., 0.],
    };
    let placed = s.place_prefab(&mut w, 5, &prefab, at_frame).unwrap();
    assert_eq!(placed.len(), 3);
    // Three adds, one label and the group.
    assert_eq!(s.document().ops.len(), before + 5);
    let group = s.group_of(at(placed[0])).unwrap();
    assert_eq!(group.members.len(), 3);
    assert_eq!(group.name, "wall");
    assert!(group.pivot_first);
    assert_eq!(
        s.label_of(at(placed[1])),
        Some(label("checkpoint", Some(3)))
    );
    // The pivot lands on the frame; the line that ran along world x at a
    // pivot yaw of 0.3 now runs at 1.0 - 0.3.
    near(s.thing(at(placed[0])).unwrap().frame, at_frame);
    let gap = s.thing(at(ids[1])).unwrap().frame.position;
    let first = s.thing(at(ids[0])).unwrap().frame.position;
    let length = ((gap[0] - first[0]).powi(2) + (gap[1] - first[1]).powi(2)).sqrt();
    let second = s.thing(at(placed[1])).unwrap().frame.position;
    let expected = [
        at_frame.position[0] + length * 0.7f32.cos(),
        at_frame.position[1] + length * 0.7f32.sin(),
    ];
    assert!(
        (second[0] - expected[0]).abs() < 1e-3 && (second[1] - expected[1]).abs() < 1e-3,
        "{second:?} is not {expected:?}"
    );
    // One undo takes all of it, and one redo puts it back.
    s.undo(&mut w, 6).unwrap();
    assert_eq!(s.document().ops.len(), before);
    assert!(s.thing(at(placed[0])).is_none());
    s.redo(&mut w, 7).unwrap();
    assert_eq!(s.document().ops.len(), before + 5);
    assert!(s.group_of(at(placed[2])).is_some());
}

#[test]
fn a_single_object_prefab_is_not_a_group() {
    let (mut s, mut w, ids) = build();
    let prefab = s.capture_prefab("one", &[at(ids[0])]).unwrap();
    assert!(!prefab.grouped);
    let placed = s.place_prefab(&mut w, 5, &prefab, frame(8., 0.)).unwrap();
    assert!(s.group_of(at(placed[0])).is_none());
}

#[test]
fn a_prefab_naming_a_tag_the_map_lacks_is_refused_by_name_and_places_nothing() {
    let (mut s, mut w, ids) = build();
    let mut prefab = s.capture_prefab("wall", &members(&ids)).unwrap();
    prefab.pieces[2].tag = TagKey {
        group: "scen".into(),
        path: r"somewhere\else".into(),
    };
    let before = s.document().ops.len();
    let error = s
        .place_prefab(&mut w, 5, &prefab, frame(8., 0.))
        .unwrap_err();
    assert!(
        error.contains("scen") && error.contains(r"somewhere\else"),
        "{error}"
    );
    assert!(s.prefab_fits(&prefab).is_err());
    assert_eq!(s.document().ops.len(), before);
    prefab.pieces[2].tag = s.palette()[0].tag.clone();
    prefab.pieces[2].block = BlockKind::Scenery;
    assert!(
        s.prefab_fits(&prefab).is_err(),
        "the block is part of the key"
    );
}

#[test]
fn a_prefab_that_does_not_fit_the_budget_is_refused_whole() {
    let mut s = session(4);
    let mut w = Fake::default();
    let ids: Vec<OpId> = [0., 2., 4.]
        .map(|x| s.place(&mut w, 1, 0, frame(x, 0.)).unwrap())
        .to_vec();
    let prefab = s.capture_prefab("three", &members(&ids)).unwrap();
    let before = s.document().ops.len();
    let error = s
        .place_prefab(&mut w, 2, &prefab, frame(8., 0.))
        .unwrap_err();
    assert!(error.contains("only 1 more fit"), "{error}");
    assert_eq!(s.document().ops.len(), before);
}

#[test]
fn only_objects_the_session_placed_are_saved() {
    let (s, _, ids) = build();
    let own = Endpoint::base(BlockKind::Crates, 0);
    assert!(s.capture_prefab("own", &[at(ids[0]), own]).is_err());
    assert!(s.capture_prefab("none", &[]).is_err());
    assert!(s.capture_prefab("bad name!", &[at(ids[0])]).is_err());
}

#[test]
fn prefab_files_are_written_listed_and_read_back_and_a_bad_one_is_named() {
    let (s, _, ids) = build();
    let dir = Files::new();
    assert_eq!(files::load_prefabs(&dir.0).unwrap(), (vec![], vec![]));
    let saved = s.save_prefab(&dir.0, "wall", &members(&ids)).unwrap();
    s.save_prefab(&dir.0, "post", &[at(ids[0])]).unwrap();
    let folder = dir.0.join("forge").join("prefabs");
    std::fs::write(folder.join("broken.h2prefab"), b"{").unwrap();
    // A file whose name is not its stem's.
    std::fs::write(
        folder.join("liar.h2prefab"),
        saved.canonical_json().unwrap(),
    )
    .unwrap();
    let (prefabs, skipped) = files::load_prefabs(&dir.0).unwrap();
    let names: Vec<&str> = prefabs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["post", "wall"]);
    assert_eq!(prefabs[1], saved);
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    assert!(skipped.iter().any(|l| l.starts_with("broken: ")));
    assert!(skipped.iter().any(|l| l.contains("holds the name wall")));
}
