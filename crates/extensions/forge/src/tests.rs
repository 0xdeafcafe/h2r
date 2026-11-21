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

#[test]
fn canonical_save_loads_at_its_document_head_and_compensates_failed_loads() {
    let data = Files::new();
    let mut a = session(3);
    let base = a.document().clone();
    let mut world = Fake::default();
    let id = a.place(&mut world, 1, 0, frame(1., 0.)).unwrap();
    a.move_object(&mut world, 2, id, frame(2., 0.2)).unwrap();
    a.place(&mut world, 3, 0, frame(5., 0.)).unwrap();
    a.save(&data.0, "our-room").unwrap();
    let bytes = fs::read(data.0.join("forge/our-room.h2forge")).unwrap();
    assert_eq!(
        bytes,
        forge_doc::fold(&base, a.log())
            .unwrap()
            .canonical_json()
            .unwrap()
            .into_bytes()
    );
    assert_eq!(saved_names(&data.0).unwrap(), vec!["our-room"]);
    let mut b = session(3);
    let mut second = Fake {
        refuse_at: Some(1),
        ..Default::default()
    };
    assert!(b.load(&mut second, 4, &data.0, "our-room").is_err());
    assert_eq!(b.document(), &base);
    assert!(second.objects.is_empty());
    assert!(b.objects().is_empty());
    b.load(&mut second, 5, &data.0, "our-room").unwrap();
    assert_eq!(b.document(), a.document());
    assert_eq!(b.head().hash, a.document().id().unwrap());
    assert!(b.log().is_empty());
    assert_eq!(
        b.objects().iter().map(|o| o.frame).collect::<Vec<_>>(),
        vec![frame(2., 0.2), frame(5., 0.)]
    );
    b.delete(&mut second, 6, id).unwrap();
    assert_eq!(
        forge_doc::fold(a.document(), b.log()).unwrap(),
        *b.document()
    );
}

#[test]
fn saves_refuse_path_components_and_linked_worktrees() {
    let data = Files::new();
    let s = session(1);
    assert!(s.save(&data.0, "../outside").is_err());
    fs::write(data.0.join(".git"), "gitdir: synthetic").unwrap();
    assert!(s.save(&data.0.join("missing/data"), "room").is_err());
    assert!(!data.0.join("missing").exists());
}

#[test]
fn save_as_allocates_unique_complete_files_concurrently() {
    let files = Files::new();
    let base = "q".repeat(64);
    let mut a = session(3);
    let b = session(3);
    a.place(&mut Fake::default(), 1, 0, frame(1., 0.)).unwrap();
    let (a_name, b_name) = std::thread::scope(|scope| {
        let first = scope.spawn(|| a.save_new(&files.0, &base).unwrap());
        let second = scope.spawn(|| b.save_new(&files.0, &base).unwrap());
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_ne!(a_name, b_name);
    assert_eq!(a_name.len(), 64);
    assert_eq!(b_name.len(), 64);
    let mut names = [a_name.clone(), b_name.clone()];
    names.sort();
    assert_eq!(saved_names(&files.0).unwrap(), names);
    for (name, doc) in [(a_name, a.document()), (b_name, b.document())] {
        assert_eq!(
            fs::read(files.0.join("forge").join(format!("{name}.h2forge"))).unwrap(),
            doc.canonical_json().unwrap().into_bytes()
        );
    }
}

#[path = "tool_tests.rs"]
mod tool_tests;
