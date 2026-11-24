//! Invented block, tag, handles and file. No map or game installation.
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::*;
use forge_doc::{
    BuilderVersions, Edit, F32, ForgeDoc, Limits, MapRef, Op, Placement, Sha256, Value, WorldBounds,
};

#[derive(Debug, PartialEq)]
enum Call {
    Place(u32, Frame),
    Move(u32, Frame),
    Delete(u32),
    Respawn(u32, Frame),
    Channel(u32, i16),
}

#[derive(Default)]
struct Fake {
    next: u32,
    objects: BTreeMap<u32, Frame>,
    calls: Vec<Call>,
    refuse_at: Option<usize>,
}

impl Fake {
    fn admit(&mut self) -> Result<(), String> {
        if self.refuse_at == Some(self.calls.len()) {
            self.refuse_at = None;
            Err("named engine refusal".into())
        } else {
            Ok(())
        }
    }
    fn create(&mut self, frame: Frame, restored: bool) -> Result<u32, String> {
        self.admit()?;
        self.next += 1;
        self.objects.insert(self.next, frame);
        self.calls.push(if restored {
            Call::Respawn(self.next, frame)
        } else {
            Call::Place(self.next, frame)
        });
        Ok(self.next)
    }
}
impl World for Fake {
    fn place(&mut self, _: &PaletteItem, f: Frame) -> Result<u32, String> {
        self.create(f, false)
    }
    fn respawn(&mut self, _: &PaletteItem, f: Frame) -> Result<u32, String> {
        self.create(f, true)
    }
    fn move_object(&mut self, h: u32, f: Frame) -> Result<(), String> {
        self.admit()?;
        *self.objects.get_mut(&h).ok_or("stale handle")? = f;
        self.calls.push(Call::Move(h, f));
        Ok(())
    }
    fn delete(&mut self, h: u32) -> Result<(), String> {
        self.admit()?;
        self.objects.remove(&h).ok_or("stale handle")?;
        self.calls.push(Call::Delete(h));
        Ok(())
    }
    fn channel(&mut self, flag: u32, channel: i16) -> Result<(), String> {
        self.admit()?;
        self.calls.push(Call::Channel(flag, channel));
        Ok(())
    }
}

fn session(budget: usize) -> Session {
    let block = BlockKind::Crates;
    let tag = TagKey {
        group: "bloc".into(),
        path: r"fixture\cube".into(),
    };
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "testroom".into(),
            sha256: Sha256::of(b"an invented room"),
        },
        sources: vec![],
        title: "Room".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    };
    let limits = Limits {
        base_counts: BTreeMap::from([(block, 7)]),
        world: Some(WorldBounds {
            min: [-20.; 3],
            max: [20.; 3],
        }),
        ..Default::default()
    };
    Session::new(base, limits, vec![PaletteItem { block, tag }], budget).unwrap()
}
fn frame(x: f32, yaw: f32) -> Frame {
    Frame {
        position: [x, 1., 2.],
        rotation: [yaw, 0., 0.],
    }
}

#[test]
fn object_gestures_stamp_portable_edits_and_call_the_world_once_each() {
    let mut session = session(2);
    let base = session.document().clone();
    let mut world = Fake::default();
    let (a, b) = (frame(1., 0.), frame(3., 0.5));
    let id = session.place(&mut world, 1, 0, a).unwrap();
    session.move_object(&mut world, 2, id, b).unwrap();
    session.delete(&mut world, 3, id).unwrap();
    let ops = &session.document().ops;
    assert_eq!(ops.len(), 4);
    let Edit::Placement(Placement::Add {
        block,
        like,
        fields,
    }) = &ops[0].edit
    else {
        panic!("expected add")
    };
    assert_eq!((*block, *like), (BlockKind::Crates, None));
    assert_eq!(
        fields[0].value,
        Value::Tag(Some(session.palette()[0].tag.clone()))
    );
    for (op, name, v) in [
        (&ops[1], "position", b.position),
        (&ops[2], "rotation", b.rotation),
    ] {
        let Edit::Placement(Placement::Set {
            block,
            index,
            write,
        }) = &op.edit
        else {
            panic!("expected set")
        };
        assert_eq!(
            (*block, *index, write.field.as_str()),
            (BlockKind::Crates, 7, name)
        );
        assert_eq!(write.value, Value::Vec3(v.map(|x| F32::new(x).unwrap())));
    }
    assert_eq!(
        ops[3].edit,
        Edit::Placement(Placement::Remove {
            block: BlockKind::Crates,
            index: 7
        })
    );
    session.undo(&mut world, 4).unwrap(); // restore with a fresh handle
    session.undo(&mut world, 5).unwrap(); // both fields in the same drop
    assert_eq!(session.objects()[0].frame, a);
    session.redo(&mut world, 6).unwrap();
    session.redo(&mut world, 7).unwrap();
    assert!(world.objects.is_empty());
    assert_eq!(
        world.calls,
        vec![
            Call::Place(1, a),
            Call::Move(1, b),
            Call::Delete(1),
            Call::Respawn(2, b),
            Call::Move(2, a),
            Call::Move(2, b),
            Call::Delete(2)
        ]
    );
    assert_eq!(
        forge_doc::fold(&base, session.log()).unwrap(),
        *session.document()
    );
}

#[test]
fn removal_swaps_the_last_placement_and_undo_restores_its_row() {
    let mut s = session(3);
    let mut w = Fake::default();
    let ids = [1., 2., 3.].map(|x| s.place(&mut w, 1, 0, frame(x, 0.)).unwrap());
    s.delete(&mut w, 2, ids[0]).unwrap();
    assert_eq!(s.objects()[0].id, ids[2]);
    s.move_object(&mut w, 3, ids[2], frame(4., 0.)).unwrap();
    assert!(matches!(
        &s.document().ops.last().unwrap().edit,
        Edit::Placement(Placement::Set { index: 7, .. })
    ));
    s.undo(&mut w, 4).unwrap();
    s.undo(&mut w, 5).unwrap();
    assert_eq!(s.objects().iter().map(|o| o.id).collect::<Vec<_>>(), ids);
}

#[test]
fn refusal_leaves_log_documents_and_world_alone() {
    let mut s = session(1);
    let mut w = Fake::default();
    let a = frame(1., 0.);
    let id = s.place(&mut w, 1, 0, a).unwrap();
    let head = s.head();
    let doc = s.document().clone();
    assert!(s.place(&mut w, 2, 0, a).is_err());
    assert!(s.move_object(&mut w, 2, id, frame(21., 0.)).is_err());
    assert!(s.move_object(&mut w, 2, id, frame(f32::NAN, 0.)).is_err());
    let edit = Placement::Set {
        block: BlockKind::Crates,
        index: 7,
        write: forge_doc::FieldWrite {
            field: "position".into(),
            value: Value::Vec3([F32::new(1.).unwrap(); 3]),
        },
    };
    let op = Op {
        id: OpId(11),
        edit: Edit::Placement(edit),
    };
    assert!(
        s.propose(&mut w, 2, op.clone(), Some(Sha256::of(b"stale")))
            .is_err()
    );
    w.refuse_at = Some(1);
    assert_eq!(s.undo(&mut w, 2).unwrap_err(), "named engine refusal");
    assert_eq!(s.document(), &doc);
    assert_eq!(s.head(), head);
    assert_eq!(w.calls, vec![Call::Place(1, a)]);
    let entry = s
        .propose(
            &mut w,
            3,
            op.clone(),
            Some(
                Value::Vec3(a.position.map(|x| F32::new(x).unwrap()))
                    .digest()
                    .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(entry.op, op);
    assert_eq!(entry.prev_hash, head.hash);
    assert!(s.propose(&mut w, 4, op, None).is_err());
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "h2-forge-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn variant_path(data: &std::path::Path, stem: &str) -> PathBuf {
    data.join("map-variants/testroom")
        .join(format!("{stem}.h2mapvariant"))
}

#[test]
fn canonical_save_loads_at_its_document_head_and_compensates_failed_loads() {
    let data = Files::new();
    let mut a = session(3);
    let base = a.document().clone();
    let mut world = Fake::default();
    let id = a.place(&mut world, 1, 0, frame(1., 0.)).unwrap();
    a.move_object(&mut world, 2, id, frame(2., 0.2)).unwrap();
    a.place(&mut world, 3, 0, frame(5., 0.)).unwrap();
    let stem = a.save_variant(&data.0, "Our Room", false).unwrap();
    assert_eq!(stem, "Our_Room");
    let bytes = fs::read(variant_path(&data.0, &stem)).unwrap();
    let mut want = forge_doc::play_form(&forge_doc::fold(&base, a.log()).unwrap());
    want.title = "Our Room".into();
    assert_eq!(bytes, want.canonical_json().unwrap().into_bytes());
    let listed = files::variants::list(&data.0, "testroom", None).unwrap();
    assert_eq!(
        listed.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        vec!["Our Room"]
    );
    assert!(saved_names(&data.0).unwrap().is_empty());

    let saved = ForgeDoc::from_bytes(&bytes, &Limits::default()).unwrap();
    let mut b = session(3);
    let mut second = Fake {
        refuse_at: Some(1),
        ..Default::default()
    };
    assert!(b.load_variant(&mut second, 4, saved.clone()).is_err());
    assert_eq!(b.document(), &base);
    assert!(second.objects.is_empty());
    assert!(b.objects().is_empty());
    b.load_variant(&mut second, 5, saved).unwrap();
    let mut expected = a.document().clone();
    expected.title = "Our Room".into();
    assert_eq!(b.document(), &expected);
    assert_eq!(b.head().hash, expected.id().unwrap());
    assert!(b.log().is_empty());
    assert_eq!(
        b.objects().iter().map(|o| o.frame).collect::<Vec<_>>(),
        vec![frame(2., 0.2), frame(5., 0.)]
    );
    b.delete(&mut second, 6, id).unwrap();
    assert_eq!(forge_doc::fold(&expected, b.log()).unwrap(), *b.document());
}

#[test]
fn saves_sanitise_names_and_refuse_linked_worktrees() {
    let data = Files::new();
    let s = session(1);
    // A name is never a path: it becomes a stem inside the map's directory.
    let stem = s.save_variant(&data.0, "../outside", false).unwrap();
    assert!(!stem.contains('/') && !stem.contains('.'));
    assert!(variant_path(&data.0, &stem).is_file());
    assert!(!data.0.join("outside").exists());
    fs::write(data.0.join(".git"), "gitdir: synthetic").unwrap();
    assert!(
        s.save_variant(&data.0.join("missing/data"), "Room", false)
            .is_err()
    );
    assert!(!data.0.join("missing").exists());
}

#[test]
fn a_taken_name_is_refused_unless_replacing() {
    let files = Files::new();
    let mut a = session(3);
    let b = session(3);
    a.place(&mut Fake::default(), 1, 0, frame(1., 0.)).unwrap();
    let first = a.save_variant(&files.0, "Arena", false).unwrap();
    assert_eq!(
        b.save_variant(&files.0, "ARENA", false),
        Err(files::variants::Error::NameTaken("ARENA".into()))
    );
    let again = b.save_variant(&files.0, "ARENA", true).unwrap();
    assert_eq!(again, first);
    let mut want = b.variant_form("ARENA");
    want.expect = None;
    assert_eq!(
        fs::read(variant_path(&files.0, &first)).unwrap(),
        want.canonical_json().unwrap().into_bytes()
    );
    assert_eq!(
        files::variants::list(&files.0, "testroom", None)
            .unwrap()
            .len(),
        1
    );
}

#[path = "tool_tests.rs"]
mod tool_tests;

#[path = "link_tests.rs"]
mod link_tests;

#[path = "group_tests.rs"]
mod group_tests;

#[path = "prefab_tests.rs"]
mod prefab_tests;
