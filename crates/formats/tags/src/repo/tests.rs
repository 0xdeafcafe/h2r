use super::format::{decode, encode};
use super::*;
use crate::schema::Schema;
use map::{CacheType, FourCC, Map};
use std::collections::BTreeSet;
use std::io::Cursor;

/// An in-memory source.
struct Mem {
    tags: Vec<LooseTag>,
    raws: HashMap<[u8; 32], Vec<u8>>,
    groups: Vec<map::TagGroup>,
}

impl TagSource for Mem {
    fn keys(&self) -> Vec<TagKey> {
        self.tags.iter().map(|t| t.key.clone()).collect()
    }
    fn read(&mut self, key: &TagKey) -> Result<Option<LooseTag>> {
        Ok(self.tags.iter().find(|t| &t.key == key).cloned())
    }
    fn raw(&mut self, raw: &RawRes) -> Result<Vec<u8>> {
        self.raws
            .get(&raw.hash)
            .cloned()
            .ok_or_else(|| Error::Invalid("no raw".into()))
    }
    fn groups(&self) -> Vec<map::TagGroup> {
        self.groups.clone()
    }
}

const SCHEMA: &str = "\
group matg 0x10 align 4
  0x0 block 0xc align 4
    0x0 tagref
    0x8 string
group scnr 0x218 align 4
  0x0 datum
  0x4 block 0x1 align 4
  0xc string
group bitm 0xc align 4
  0x0 raw
  0x4 size 0x0
";

fn g(s: &[u8; 4]) -> FourCC {
    FourCC(*s)
}

fn sample() -> Mem {
    let pixels: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
    let hash = sha256(&pixels);
    let bitm = LooseTag {
        key: TagKey::new(g(b"bitm"), "a\\pixels"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: {
                    let mut b = vec![0u8; 12];
                    b[4..8].copy_from_slice(&300u32.to_le_bytes());
                    b[8] = 9;
                    b
                },
                count: 1,
                size: 12,
                align: 4,
                fields: vec![(0, Value::Raw(0))],
            }],
            raws: vec![RawRes {
                origin: Origin::Local,
                len: 300,
                hash,
            }],
            losses: Vec::new(),
        }),
    };
    // Two elements, both naming the bitmap, each with a string.
    let mut elems = vec![0u8; 24];
    for e in [0usize, 12] {
        elems[e..e + 4].copy_from_slice(&g(b"bitm").as_u32().to_le_bytes());
    }
    let matg = LooseTag {
        key: TagKey::new(g(b"matg"), "globals\\globals"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![
                Node {
                    bytes: {
                        let mut b = vec![0u8; 16];
                        b[0] = 2;
                        b[12] = 0x5a;
                        b
                    },
                    count: 1,
                    size: 16,
                    align: 4,
                    fields: vec![(0, Value::Block(1))],
                },
                Node {
                    bytes: elems,
                    count: 2,
                    size: 12,
                    align: 4,
                    fields: vec![
                        (4, Value::Ref(TagKey::new(g(b"bitm"), "a\\pixels"))),
                        (8, Value::String("first".into())),
                        (16, Value::Ref(TagKey::new(g(b"bitm"), "a\\pixels"))),
                        (20, Value::String("second".into())),
                    ],
                },
            ],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    };
    let scnr = LooseTag {
        key: TagKey::new(g(b"scnr"), "levels\\test\\test"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![
                Node {
                    bytes: {
                        let mut b = vec![0u8; 0x218];
                        b[4] = 5;
                        b[16] = 1;
                        b
                    },
                    count: 1,
                    size: 0x218,
                    align: 4,
                    fields: vec![
                        (0, Value::Ref(TagKey::new(g(b"matg"), "globals\\globals"))),
                        (4, Value::Data(1)),
                        (12, Value::String("first".into())),
                    ],
                },
                Node {
                    bytes: b"hello".to_vec(),
                    count: 5,
                    size: 1,
                    align: 4,
                    fields: Vec::new(),
                },
            ],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    };
    let alias = LooseTag {
        key: TagKey::new(g(b"bitm"), "a\\same_pixels"),
        body: Body::Alias {
            owner: TagKey::new(g(b"bitm"), "a\\pixels"),
            offset: 0,
        },
    };
    let groups = [b"matg", b"scnr", b"bitm"]
        .iter()
        .map(|c| map::TagGroup {
            class: g(c),
            parent: None,
            grandparent: None,
        })
        .collect();
    Mem {
        tags: vec![matg, scnr, bitm, alias],
        raws: [(hash, pixels)].into_iter().collect(),
        groups,
    }
}

#[test]
fn tag_files_round_trip() {
    for t in sample().tags {
        let bytes = encode(&t);
        assert_eq!(decode(&bytes).unwrap(), t, "{}", t.key);
        // Deterministic.
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }
}

#[test]
fn bad_tag_files_are_rejected() {
    let t = &sample().tags[0];
    let bytes = encode(t);
    assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    let mut v = bytes.clone();
    v[4] = 9;
    assert!(decode(&v).is_err(), "unknown version");
    assert!(decode(b"nope").is_err());
}

#[test]
fn closure_follows_references_and_reports_missing() {
    let mut src = sample();
    let scnr = TagKey::new(g(b"scnr"), "levels\\test\\test");
    let c = closure(&mut src, std::slice::from_ref(&scnr)).unwrap();
    let keys: Vec<String> = c.tags.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(
        keys,
        [
            "levels\\test\\test.scnr",
            "globals\\globals.matg",
            "a\\pixels.bitm"
        ]
    );
    assert!(c.missing.is_empty());
    src.tags.retain(|t| t.key.group != g(b"bitm"));
    let c = closure(&mut src, &[scnr]).unwrap();
    assert_eq!(c.missing.len(), 1);
}

fn options() -> BuildOptions {
    BuildOptions {
        cache_type: CacheType::Multiplayer,
        name: "test".into(),
        scenario: TagKey::new(g(b"scnr"), "levels\\test\\test"),
        globals: TagKey::new(g(b"matg"), "globals\\globals"),
        strings: vec![Some("engine".into())],
        header_template: None,
        low_detail: None,
        index_word_14: 0,
    }
}

/// Build a map from loose tags, read it back with the schema, and get the
/// same tags out.
#[test]
fn loose_tags_build_and_extract_the_same() {
    let schema = Schema::parse(SCHEMA).unwrap();
    let mut src = sample();
    let tags = src.tags.clone();
    let (set, report) = to_tag_set(&mut src, &tags, &options()).unwrap();
    assert_eq!(report.tags, 4);
    assert_eq!(report.raw, (1, 300));
    assert!(report.dangling.is_empty());
    let (bytes, _) = set.build().unwrap();
    let map = Map::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(map.strings()[0].as_deref(), Some("engine"));
    let mut shared = SharedFiles::default();
    let mut back = MapSource::new(&schema, map, &mut shared).unwrap();
    for t in &tags {
        let got = back.read(&t.key).unwrap().expect("present");
        // The raw length comes from the paired size: equal content.
        assert_eq!(canonical(&got), canonical(t), "{}", t.key);
        if let Some(tree) = got.tree() {
            for r in &tree.raws {
                assert_eq!(back.raw(r).unwrap(), src.raw(r).unwrap());
            }
        }
    }
}

#[test]
fn a_repository_stores_variants_and_reads_them_per_map() {
    let dir = std::env::temp_dir().join(format!("h2-tagrepo-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut repo = TagRepo::create(&dir).unwrap();
    let src = sample();
    let t = src.tags[0].clone();
    assert_eq!(repo.put(&t).unwrap(), store::Stored::New);
    assert_eq!(repo.put(&t).unwrap(), store::Stored::Same);
    let mut other = t.clone();
    if let Body::Tree(tr) = &mut other.body {
        tr.nodes[0].bytes[12] = 0x77;
    }
    let store::Stored::Variant(v) = repo.put(&other).unwrap() else {
        panic!("not a variant");
    };
    let m = store::Manifest {
        map: "two".into(),
        tags: vec![(other.key.clone(), Some(v))],
        ..Default::default()
    };
    repo.put_manifest(&m, &[0; 0x800]).unwrap();
    assert_eq!(repo.read(&t.key).unwrap().unwrap(), t);
    let mut two = TagRepo::for_map(&dir, "two").unwrap();
    assert_eq!(two.read(&t.key).unwrap().unwrap(), other);
    assert_eq!(repo.scan().unwrap(), vec![t.key.clone()]);
    let h = repo.put_raw(b"abc").unwrap();
    assert_eq!(repo.raw_by_hash(&h).unwrap(), b"abc");
    std::fs::remove_dir_all(&dir).unwrap();
}

// --- Extraction versions (`extract.toml`, the plan, `--update`) ---

/// A temporary directory, removed when dropped.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("h2-tagrepo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        TempDir(d)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The test schema with one more field in `matg`'s root: only `matg`'s
/// layout hash moves, and what it extracts is the same.
fn schema_with_matg_word() -> Schema {
    let text = SCHEMA.replace("    0x8 string\n", "    0x8 string\n  0x8 word\n");
    assert_ne!(text, SCHEMA);
    Schema::parse(&text).unwrap()
}

/// An invented map file: the sample tags, with `tweak` in the globals'
/// bytes so maps can differ.
fn invented_map(tweak: u8) -> Vec<u8> {
    let mut src = sample();
    if let Body::Tree(t) = &mut src.tags[0].body {
        t.nodes[0].bytes[12] = tweak;
    }
    let tags = src.tags.clone();
    let (set, _) = to_tag_set(&mut src, &tags, &options()).unwrap();
    set.build().unwrap().0
}

/// An invented disc: stem → file bytes.
type Disc = BTreeMap<String, Vec<u8>>;

fn disc(maps: &[(&str, u8)]) -> Disc {
    maps.iter()
        .map(|(s, t)| ((*s).to_owned(), invented_map(*t)))
        .collect()
}

fn identity(d: &Disc) -> Vec<(String, String)> {
    d.iter()
        .map(|(s, b)| (s.clone(), extract::file_hash(Cursor::new(b)).unwrap()))
        .collect()
}

/// Plan and update over an invented disc. Opening a stem in `fail` fails
/// (an interrupted run). Returns the plan and, per map done, the number of
/// tags it stored.
fn run_update(
    repo: &mut TagRepo,
    schema: &Schema,
    d: &Disc,
    fail: &[&str],
) -> (extract::Plan, Result<Vec<(String, usize)>>) {
    let layouts = extract::Layouts::new(schema);
    let plan = extract::plan(repo, &layouts, &identity(d), false).unwrap();
    let mut shared = SharedFiles::<Cursor<Vec<u8>>>::default();
    let mut open = |stem: &str| -> Result<Map<Cursor<Vec<u8>>>> {
        if fail.contains(&stem) {
            return Err(Error::Invalid(format!("interrupted at {stem}")));
        }
        Ok(Map::from_reader(Cursor::new(d[stem].clone()))?)
    };
    let mut done = Vec::new();
    let mut on_map = |w: &extract::MapWork, r: &store::PutReport| {
        done.push((w.map.clone(), r.new + r.same + r.variants.len()));
    };
    let r = extract::update(
        repo,
        &plan,
        &layouts,
        schema,
        &mut shared,
        &mut open,
        false,
        &mut on_map,
    );
    (plan, r.map(|_| done))
}

type Snapshot = Vec<(std::path::PathBuf, Vec<u8>, std::time::SystemTime)>;

/// Every file under `dir` with its bytes and modification time.
fn snapshot(dir: &std::path::Path) -> Snapshot {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                todo.push(p);
            } else {
                let t = e.metadata().unwrap().modified().unwrap();
                out.push((p.clone(), std::fs::read(&p).unwrap(), t));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_extraction_record_round_trips() {
    let mut r = store::ExtractRecord {
        extractor: 3,
        format: 1,
        ..Default::default()
    };
    let mut m = store::MapRecord {
        file: "ab".repeat(32),
        ..Default::default()
    };
    for (group, lossy) in [
        (*b"scnr", 1),
        (*b"snd!", 0),
        (*b"<fx>", 0),
        ([0x20, 0, 1, 2], 0),
    ] {
        m.groups.insert(
            FourCC(group),
            store::GroupRecord {
                layout: "0f".repeat(32),
                tags: 7,
                lossy,
                refused: 0,
            },
        );
    }
    r.maps.insert("lockout".into(), m.clone());
    r.maps.insert("odd \"name\"".into(), m);
    let text = r.to_text();
    assert_eq!(store::ExtractRecord::parse(&text).unwrap(), r, "{text}");
    assert!(store::ExtractRecord::parse("extractor = x\n").is_err());
    assert_eq!(r.maps["lockout"].lossy(), 1);
}

#[test]
fn a_layout_hash_moves_with_its_group_only() {
    let a = Schema::parse(SCHEMA).unwrap();
    let b = schema_with_matg_word();
    let (la, lb) = (extract::Layouts::new(&a), extract::Layouts::new(&b));
    assert_ne!(la.hash(g(b"matg")), lb.hash(g(b"matg")));
    assert_eq!(la.hash(g(b"scnr")), lb.hash(g(b"scnr")));
    assert_eq!(la.hash(g(b"bitm")), lb.hash(g(b"bitm")));
    // A group gains a layout: its hash moves. A new raw field moves every
    // group with raw fields (their lengths run to the next raw start), and
    // no other.
    let c = Schema::parse(&format!("{SCHEMA}group weap 0x4 align 4\n  0x0 raw\n")).unwrap();
    let lc = extract::Layouts::new(&c);
    assert_ne!(la.hash(g(b"weap")), lc.hash(g(b"weap")));
    assert_ne!(la.hash(g(b"bitm")), lc.hash(g(b"bitm")));
    assert_eq!(la.hash(g(b"matg")), lc.hash(g(b"matg")));
    assert_eq!(la.hash(g(b"scnr")), lc.hash(g(b"scnr")));
}

/// Unchanged input is a no-op: after a full extraction the plan is empty
/// and running it writes nothing.
#[test]
fn unchanged_input_is_a_no_op() {
    let dir = TempDir::new("noop");
    let schema = Schema::parse(SCHEMA).unwrap();
    let d = disc(&[("alpha", 0x5a), ("beta", 0x77)]);
    let mut repo = TagRepo::create(&dir.0).unwrap();
    let (plan, done) = run_update(&mut repo, &schema, &d, &[]);
    assert!(plan.reset);
    assert_eq!(
        done.unwrap(),
        [("alpha".to_owned(), 4), ("beta".to_owned(), 4)]
    );
    let rec = repo.extract_record().unwrap().unwrap();
    assert_eq!(rec.extractor, extract::EXTRACT_VERSION);
    assert_eq!(rec.maps.len(), 2);
    assert_eq!(rec.maps["alpha"].groups[&g(b"bitm")].tags, 2);
    // beta's globals differ from alpha's: a variant, listed.
    let conflicts = std::fs::read_to_string(dir.0.join("conflicts.txt")).unwrap();
    assert_eq!(conflicts, "globals\\globals.matg\tbeta\n");

    let before = snapshot(&dir.0);
    // Opening any map would fail: nothing may be opened.
    let (plan, done) = run_update(&mut repo, &schema, &d, &["alpha", "beta"]);
    assert!(plan.is_empty(), "{plan:?}");
    assert!(done.unwrap().is_empty());
    assert_eq!(snapshot(&dir.0), before, "an empty plan wrote nothing");
}

/// An interrupted run redoes the map it was in, and only that map.
#[test]
fn an_interrupted_run_redoes_one_map() {
    let dir = TempDir::new("resume");
    let schema = Schema::parse(SCHEMA).unwrap();
    let d = disc(&[("alpha", 1), ("beta", 2), ("gamma", 3)]);
    let mut repo = TagRepo::create(&dir.0).unwrap();
    let (plan, done) = run_update(&mut repo, &schema, &d, &["gamma"]);
    assert_eq!(plan.maps.len(), 3);
    assert!(done.is_err(), "stopped at gamma");
    let rec = repo.extract_record().unwrap().unwrap();
    assert_eq!(rec.maps.keys().collect::<Vec<_>>(), ["alpha", "beta"]);

    let (plan, done) = run_update(&mut repo, &schema, &d, &[]);
    assert!(!plan.reset);
    let redo: Vec<_> = plan
        .maps
        .iter()
        .map(|w| (w.map.as_str(), w.why.clone(), w.groups.is_none()))
        .collect();
    assert_eq!(redo, [("gamma", extract::Why::New, true)]);
    assert_eq!(done.unwrap(), [("gamma".to_owned(), 4)]);
    assert!(run_update(&mut repo, &schema, &d, &[]).0.is_empty());
    // Each map reads its own globals.
    for (stem, tweak) in [("alpha", 1u8), ("beta", 2), ("gamma", 3)] {
        let mut r = TagRepo::for_map(&dir.0, stem).unwrap();
        let t = r
            .read(&TagKey::new(g(b"matg"), "globals\\globals"))
            .unwrap()
            .unwrap();
        assert_eq!(t.tree().unwrap().nodes[0].bytes[12], tweak, "{stem}");
    }
    // A changed file is taken whole again, alone.
    let mut d2 = d.clone();
    d2.insert("beta".into(), invented_map(9));
    let (plan, done) = run_update(&mut repo, &schema, &d2, &[]);
    let redo: Vec<_> = plan
        .maps
        .iter()
        .map(|w| (w.map.as_str(), w.why.clone()))
        .collect();
    assert_eq!(redo, [("beta", extract::Why::FileChanged)]);
    assert_eq!(done.unwrap(), [("beta".to_owned(), 4)]);
}

/// A moved layout hash re-extracts that group only, in every map that has
/// it; then the plan is empty again.
#[test]
fn a_moved_layout_hash_re_extracts_that_group_only() {
    let dir = TempDir::new("layout");
    let a = Schema::parse(SCHEMA).unwrap();
    let d = disc(&[("alpha", 1), ("beta", 2)]);
    let mut repo = TagRepo::create(&dir.0).unwrap();
    run_update(&mut repo, &a, &d, &[]).1.unwrap();
    let before = repo.extract_record().unwrap().unwrap();

    let b = schema_with_matg_word();
    let (plan, done) = run_update(&mut repo, &b, &d, &[]);
    assert!(!plan.reset);
    assert_eq!(plan.maps.len(), 2);
    for w in &plan.maps {
        assert_eq!(w.why, extract::Why::Layouts);
        assert_eq!(w.groups, Some(BTreeSet::from([g(b"matg")])), "{}", w.map);
    }
    // One matg tag per map taken, nothing else.
    assert_eq!(
        done.unwrap(),
        [("alpha".to_owned(), 1), ("beta".to_owned(), 1)]
    );
    let after = repo.extract_record().unwrap().unwrap();
    let lb = extract::Layouts::new(&b);
    for stem in ["alpha", "beta"] {
        let (old, new) = (&before.maps[stem], &after.maps[stem]);
        assert_eq!(new.groups[&g(b"matg")].layout, lb.hash(g(b"matg")));
        assert_ne!(
            new.groups[&g(b"matg")].layout,
            old.groups[&g(b"matg")].layout
        );
        for k in [g(b"scnr"), g(b"bitm")] {
            assert_eq!(new.groups[&k], old.groups[&k], "{stem} {k}");
        }
        // The manifest still lists every tag.
        assert_eq!(repo.manifest(stem).unwrap().tags.len(), 4);
    }
    assert!(run_update(&mut repo, &b, &d, &[]).0.is_empty());
}

/// Settling makes each key's main file the first map's content and
/// leaves every map reading what it read.
#[test]
fn settling_puts_the_first_content_in_the_main_file() {
    let dir = TempDir::new("settle");
    let mut repo = TagRepo::create(&dir.0).unwrap();
    let t = sample().tags[0].clone();
    let mut other = t.clone();
    if let Body::Tree(tr) = &mut other.body {
        tr.nodes[0].bytes[12] = 0x77;
    }
    // beta's content stored first; alpha (first in order) has a variant.
    assert_eq!(repo.put(&t).unwrap(), store::Stored::New);
    let store::Stored::Variant(v) = repo.put(&other).unwrap() else {
        panic!("not a variant");
    };
    for (stem, variant) in [("alpha", Some(v)), ("beta", None)] {
        let m = store::Manifest {
            map: stem.into(),
            tags: vec![(t.key.clone(), variant)],
            ..Default::default()
        };
        repo.put_manifest(&m, &[0; 0x800]).unwrap();
    }
    let s = repo.settle().unwrap();
    assert_eq!(s.promoted, 1);
    assert_eq!(repo.manifest("alpha").unwrap().tags[0].1, None);
    assert!(repo.manifest("beta").unwrap().tags[0].1.is_some());
    let read = |r: Result<TagRepo>| r.unwrap().read(&t.key).unwrap().unwrap();
    assert_eq!(read(TagRepo::open(&dir.0)), other);
    assert_eq!(read(TagRepo::for_map(&dir.0, "alpha")), other);
    assert_eq!(read(TagRepo::for_map(&dir.0, "beta")), t);
    // Settled: again writes nothing.
    let before = snapshot(&dir.0);
    assert_eq!(repo.settle().unwrap().promoted, 0);
    assert_eq!(snapshot(&dir.0), before);
}
