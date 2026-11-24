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

// --- Skeletons and extent tables (docs/engine/asset-store.md, AS1) ---

use skeleton::{Extents, Part, Skeleton, StoreFile, partition};
use std::io::{Read as _, Seek as _};

fn h(n: u8) -> String {
    hex(&[n; 32])
}

/// Structural ranges are merged; a named resource inside the skeleton, or
/// overlapping one taken before it, is left to the unnamed ranges; what
/// nothing names is unnamed, split at the piece size; the parts tile.
#[test]
fn partition_tiles_the_file() {
    let len = 0x10000;
    let structural = [
        (0, 0x800),
        (0x4000, 0x1000),  // BSP 1
        (0x6000, 0x800),   // BSP 2
        (0xc000, 0x2000),  // index and metadata
        (0xc800, 0x100),   // a string table inside it
        (0xf000, 0x10000), // clipped at the end
    ];
    let named = [
        (0x1000, 0x400, h(1)),
        (0x1000, 0x200, h(2)), // same start, shorter: the longer wins
        (0x1200, 0x100, h(3)), // inside the one taken
        (0x4800, 0x100, h(4)), // inside a BSP region
        (0x2000, 0x800, h(5)),
        (0xfff0, 0x100, h(6)), // past the end
    ];
    let parts = partition(len, &structural, &named);
    assert_eq!(
        parts,
        [
            Part::Skeleton {
                offset: 0,
                len: 0x800
            },
            Part::Unnamed {
                offset: 0x800,
                len: 0x800
            },
            Part::Named {
                offset: 0x1000,
                len: 0x400,
                hash: h(1)
            },
            Part::Unnamed {
                offset: 0x1400,
                len: 0xc00
            },
            Part::Named {
                offset: 0x2000,
                len: 0x800,
                hash: h(5)
            },
            Part::Unnamed {
                offset: 0x2800,
                len: 0x1800
            },
            Part::Skeleton {
                offset: 0x4000,
                len: 0x1000
            },
            Part::Unnamed {
                offset: 0x5000,
                len: 0x1000
            },
            Part::Skeleton {
                offset: 0x6000,
                len: 0x800
            },
            Part::Unnamed {
                offset: 0x6800,
                len: 0x5800
            },
            Part::Skeleton {
                offset: 0xc000,
                len: 0x2000
            },
            Part::Unnamed {
                offset: 0xe000,
                len: 0x1000
            },
            Part::Skeleton {
                offset: 0xf000,
                len: 0x1000
            },
        ]
    );
    // A long unnamed range is split.
    let big = skeleton::UNNAMED_PIECE * 2 + 5;
    let parts = partition(big, &[], &[]);
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2].range(), (skeleton::UNNAMED_PIECE * 2, 5));
}

#[test]
fn skeletons_and_extent_tables_round_trip() {
    let s = Skeleton {
        len: 52,
        ranges: vec![(0, 4), (50, 2)],
        bytes: vec![1, 2, 3, 4, 9, 9],
    };
    assert_eq!(Skeleton::decode(&s.encode().unwrap()).unwrap(), s);
    assert!(Skeleton::decode(b"h2skel\0\x01garbage").is_err());
    assert!(Skeleton::decode(b"nope").is_err());
    let x = Extents {
        file: h(7),
        len: 52,
        extents: vec![skeleton::Extent {
            offset: 4,
            len: 46,
            hash: h(8),
        }],
    };
    assert_eq!(Extents::parse(&x.to_text()).unwrap(), x);
    assert!(Extents::parse("h2 extents 1\n4\t46\tnot-a-hash\n").is_err());
    assert!(Extents::parse("something else\n").is_err());
    // Parts that do not tile the file are refused.
    let raw = std::env::temp_dir();
    let f = StoreFile::from_parts("m", s.clone(), x.clone(), raw.clone()).unwrap();
    assert_eq!((f.len(), f.skeleton_bytes(), f.extent_count()), (52, 6, 1));
    let mut gap = x.clone();
    gap.extents[0].len = 45;
    assert!(StoreFile::from_parts("m", s.clone(), gap, raw.clone()).is_err());
    let mut overlap = x.clone();
    overlap.extents[0].offset = 3;
    assert!(StoreFile::from_parts("m", s.clone(), overlap, raw.clone()).is_err());
    let mut longer = x;
    longer.len = 53;
    assert!(StoreFile::from_parts("m", s, longer, raw).is_err());
}

/// An invented map file: a header, two BSP regions, an index with its
/// metadata, local raw resources (two at one offset), a range a pointer
/// into a shared cache would name (not local, so nothing here names it),
/// padding, and bytes past the declared size.
fn invented_file() -> (Vec<u8>, Vec<(u64, u64)>, Vec<(u64, u64)>) {
    let len = 0x9123usize;
    let bytes: Vec<u8> = (0..len)
        .map(|i| (i.wrapping_mul(31) ^ (i >> 7)) as u8)
        .collect();
    let structural = vec![
        (0, 0x800),
        (0x3000, 0x900),  // BSP 1
        (0x4000, 0x700),  // BSP 2
        (0x7000, 0x1800), // index and metadata
    ];
    // Local resources: (offset, length).
    let raws = vec![
        (0x800, 0x300),
        (0x800, 0x100),
        (0x1000, 0x1234),
        (0x5000, 0x10),
    ];
    (bytes, structural, raws)
}

fn store_invented(dir: &std::path::Path) -> (TagRepo, Vec<u8>, skeleton::SkeletonReport) {
    let (bytes, structural, raws) = invented_file();
    let mut repo = TagRepo::create(dir).unwrap();
    let mut named = Vec::new();
    for (o, l) in raws {
        let b = &bytes[o as usize..(o + l) as usize];
        named.push((o, l, repo.put_raw(b).unwrap()));
    }
    let file = hex(&sha256(&bytes));
    let parts = partition(bytes.len() as u64, &structural, &named);
    let mut read =
        |o: u64, l: u64| -> Result<Vec<u8>> { Ok(bytes[o as usize..(o + l) as usize].to_vec()) };
    let report = repo
        .put_skeleton("level", &file, bytes.len() as u64, &parts, &mut read)
        .unwrap();
    (repo, bytes, report)
}

/// Reads across extent borders equal the file at every offset class: the
/// start and end of every part, across each border, the whole file, and
/// past the end.
#[test]
fn a_store_file_reassembles_an_invented_map() {
    let dir = TempDir::new("skel-reassemble");
    let (repo, bytes, report) = store_invented(&dir.0);
    let len = bytes.len() as u64;
    assert_eq!(report.len, len);
    assert_eq!(report.skeleton.0, 0x800 + 0x900 + 0x700 + 0x1800);
    assert_eq!(report.named, (3, 0x300 + 0x1234 + 0x10));
    assert_eq!(
        report.skeleton.0 + report.named.1 + report.unnamed.1,
        len,
        "every byte is in one part"
    );
    assert!(repo.has_skeleton("level"));
    let mut f = repo.store_file("level").unwrap();
    assert_eq!(f.len(), len);
    let mut borders = vec![0u64, len];
    for p in partition(len, &invented_file().1, &[]) {
        borders.push(p.range().0);
    }
    for e in StoreFile::extents(&dir.0, "level").unwrap().extents {
        borders.extend([e.offset, e.offset + e.len]);
    }
    for b in borders {
        for start in [b.saturating_sub(3), b, b + 1] {
            for n in [1usize, 7, 0x401] {
                let mut buf = vec![0u8; n];
                let got = f.read_at(start, &mut buf).unwrap();
                let want = (len.saturating_sub(start) as usize).min(n);
                assert_eq!(got, want, "at {start:#x} +{n}");
                assert_eq!(
                    &buf[..got],
                    &bytes[start.min(len) as usize..start.min(len) as usize + got],
                    "at {start:#x} +{n}"
                );
            }
        }
    }
    let mut whole = vec![0u8; bytes.len() + 10];
    assert_eq!(f.read_at(0, &mut whole).unwrap(), bytes.len());
    assert_eq!(&whole[..bytes.len()], &bytes[..]);
    assert_eq!(f.read_at(len, &mut whole).unwrap(), 0);
    // Read and Seek, as a map reader uses them.
    let mut back = Vec::new();
    f.seek(std::io::SeekFrom::Start(0)).unwrap();
    f.read_to_end(&mut back).unwrap();
    assert_eq!(back, bytes);
    // reassemble and its check.
    let mut out = Vec::new();
    let hash = repo.reassemble("level", &mut out).unwrap();
    assert_eq!(out, bytes);
    assert_eq!(hash, hex(&sha256(&bytes)));
    repo.check_reassembly("level", &hash).unwrap();
    assert!(repo.check_reassembly("level", &h(0)).is_err());
    let st = skeleton::status(&dir.0);
    assert!(st.exists);
    assert_eq!(st.maps.len(), 1);
    assert_eq!(st.maps[0].len, len);
    assert_eq!(st.maps[0].file, hash);
}

/// A raw file the extent table names that is gone is a named error.
#[test]
fn a_missing_raw_file_is_a_named_error() {
    let dir = TempDir::new("skel-missing");
    let (repo, bytes, _) = store_invented(&dir.0);
    let gone = hex(&sha256(&bytes[0x1000..0x1000 + 0x1234]));
    std::fs::remove_file(dir.0.join("raw").join(&gone[..2]).join(&gone)).unwrap();
    let mut f = repo.store_file("level").unwrap();
    // Bytes before it still read.
    let mut buf = [0u8; 16];
    assert_eq!(f.read_at(0, &mut buf).unwrap(), 16);
    match f.read_at(0x1100, &mut buf) {
        Err(Error::MissingRaw {
            map,
            hash,
            offset,
            len,
        }) => {
            assert_eq!((map.as_str(), offset, len), ("level", 0x1000, 0x1234));
            assert_eq!(hash, gone);
        }
        other => panic!("{other:?}"),
    }
    let e = repo.reassemble("level", &mut std::io::sink()).unwrap_err();
    assert!(e.to_string().contains(&gone), "{e}");
    // A raw file of the wrong length is refused, naming it.
    std::fs::write(dir.0.join("raw").join(&gone[..2]).join(&gone), b"short").unwrap();
    let mut f = repo.store_file("level").unwrap();
    let e = f.read_at(0x1100, &mut buf).unwrap_err();
    assert!(e.to_string().contains(&gone), "{e}");
}

/// Extraction writes each map's skeleton and extent table and checks the
/// reassembled file; a map lacking them is taken again whole.
#[test]
fn extraction_writes_skeletons_and_the_plan_retakes_a_map_lacking_one() {
    let dir = TempDir::new("skel-extract");
    let schema = Schema::parse(SCHEMA).unwrap();
    let mut d = disc(&[("alpha", 1), ("beta", 2)]);
    // Bytes past beta's declared size: an unnamed range.
    d.get_mut("beta").unwrap().extend_from_slice(&[0xab; 0x123]);
    let mut repo = TagRepo::create(&dir.0).unwrap();
    run_update(&mut repo, &schema, &d, &[]).1.unwrap();
    for (stem, bytes) in &d {
        assert!(repo.has_skeleton(stem));
        let mut out = Vec::new();
        let hash = repo.reassemble(stem, &mut out).unwrap();
        assert_eq!(&out, bytes, "{stem}");
        assert_eq!(hash, extract::file_hash(Cursor::new(bytes)).unwrap());
        // The bitmap's pixels are a named extent.
        let x = StoreFile::extents(&dir.0, stem).unwrap();
        let pixels: Vec<u8> = (0..300u32).map(|i| (i * 7) as u8).collect();
        assert!(
            x.extents.iter().any(|e| e.hash == hex(&sha256(&pixels))),
            "{stem}"
        );
    }
    std::fs::remove_file(dir.0.join("maps").join("beta.extents")).unwrap();
    let (plan, done) = run_update(&mut repo, &schema, &d, &[]);
    let redo: Vec<_> = plan
        .maps
        .iter()
        .map(|w| (w.map.as_str(), w.why.clone(), w.groups.is_none()))
        .collect();
    assert_eq!(redo, [("beta", extract::Why::Skeleton, true)]);
    done.unwrap();
    assert!(repo.has_skeleton("beta"));
    assert!(run_update(&mut repo, &schema, &d, &[]).0.is_empty());
}

// --- Extraction on compile (`compile::prepare`) ---

use compile::{DiscMap, Input, Options};

/// An invented disc on disk: each map is `<stem>.map` in `dir`.
fn disc_on_disk(dir: &std::path::Path, maps: &[(&str, Vec<u8>)]) -> Vec<DiscMap> {
    std::fs::create_dir_all(dir).unwrap();
    maps.iter()
        .map(|(stem, bytes)| {
            let path = dir.join(format!("{stem}.map"));
            std::fs::write(&path, bytes).unwrap();
            DiscMap {
                stem: (*stem).to_owned(),
                path,
                base: 0,
                len: bytes.len() as u64,
            }
        })
        .collect()
}

/// `prepare` and the maps it did, in order.
fn prepare_run(
    root: &std::path::Path,
    schema: &Schema,
    disc: Option<&[DiscMap]>,
) -> Result<(compile::Prepared, Vec<String>)> {
    let mut done = Vec::new();
    let p = compile::prepare(
        root,
        schema,
        disc,
        &Options::default(),
        &mut |_| {},
        &mut |_, _| {},
        &mut |w, _| done.push(w.map.clone()),
    )?;
    Ok((p, done))
}

fn plan_maps(plan: &extract::Plan) -> Vec<(&str, extract::Why)> {
    plan.maps
        .iter()
        .map(|w| (w.map.as_str(), w.why.clone()))
        .collect()
}

fn reassembled(root: &std::path::Path, stem: &str) -> Vec<u8> {
    let mut out = Vec::new();
    TagRepo::create(root)
        .unwrap()
        .reassemble(stem, &mut out)
        .unwrap();
    out
}

/// Unchanged input opens no map and reads no map file: the stamps give
/// the hashes. A touched file (same bytes, new time) is hashed once and
/// plans nothing; a changed one is hashed once and taken alone.
#[test]
fn compile_trusts_stamps_and_takes_a_changed_map_alone() {
    let dir = TempDir::new("compile-stamps");
    let schema = Schema::parse(SCHEMA).unwrap();
    let disc = disc_on_disk(
        &dir.0.join("disc"),
        &[("alpha", invented_map(1)), ("beta", invented_map(2))],
    );
    let root = dir.0.join("store");
    let (p, done) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert_eq!((p.input, p.hashed, p.opened), (Input::Disc, 2, 2));
    assert_eq!(done, ["alpha", "beta"]);

    let before = snapshot(&root);
    let (p, done) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert!(p.plan.is_empty() && p.updated.is_none());
    assert_eq!((p.hashed, p.opened), (0, 0));
    assert!(done.is_empty());
    assert_eq!(snapshot(&root), before, "an empty plan wrote nothing");

    // Touched: the time moves, the bytes do not.
    let f = std::fs::File::options()
        .write(true)
        .open(&disc[1].path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);
    let (p, _) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert!(p.plan.is_empty());
    assert_eq!((p.hashed, p.opened), (1, 0));
    assert_eq!(prepare_run(&root, &schema, Some(&disc)).unwrap().0.hashed, 0);

    // Changed: beta alone, hashed once, taken whole.
    let mut disc = disc;
    let nine = invented_map(9);
    std::fs::write(&disc[1].path, &nine).unwrap();
    disc[1].len = nine.len() as u64;
    let (p, done) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert_eq!(plan_maps(&p.plan), [("beta", extract::Why::FileChanged)]);
    assert_eq!((p.hashed, p.opened), (1, 1));
    assert_eq!(done, ["beta"]);
    for d in &disc {
        assert_eq!(
            reassembled(&root, &d.stem),
            std::fs::read(&d.path).unwrap(),
            "{}",
            d.stem
        );
    }
    assert!(
        prepare_run(&root, &schema, Some(&disc))
            .unwrap()
            .0
            .plan
            .is_empty()
    );
}

/// A run stopped at a map the disc could not give is finished by the
/// next one, which takes only that map.
#[test]
fn compile_resumes_an_interrupted_run() {
    let dir = TempDir::new("compile-resume");
    let schema = Schema::parse(SCHEMA).unwrap();
    let mut maps = vec![
        ("alpha", invented_map(1)),
        ("beta", invented_map(2)),
        ("gamma", vec![0u8; 64]),
    ];
    let disc = disc_on_disk(&dir.0.join("disc"), &maps);
    let root = dir.0.join("store");
    assert!(
        prepare_run(&root, &schema, Some(&disc)).is_err(),
        "gamma is not a map"
    );
    let rec = TagRepo::create(&root)
        .unwrap()
        .extract_record()
        .unwrap()
        .unwrap();
    assert_eq!(rec.maps.keys().collect::<Vec<_>>(), ["alpha", "beta"]);

    maps[2].1 = invented_map(3);
    let disc = disc_on_disk(&dir.0.join("disc"), &maps);
    let (p, done) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert!(!p.plan.reset);
    assert_eq!(plan_maps(&p.plan), [("gamma", extract::Why::New)]);
    assert_eq!(done, ["gamma"]);
    assert!(
        prepare_run(&root, &schema, Some(&disc))
            .unwrap()
            .0
            .plan
            .is_empty()
    );
}

/// With no disc the maps come from the store: a moved layout hash
/// re-extracts that group from the skeleton and extents.
#[test]
fn compile_re_extracts_a_moved_layout_from_the_store() {
    let dir = TempDir::new("compile-store-layout");
    let a = Schema::parse(SCHEMA).unwrap();
    let b = schema_with_matg_word();
    let disc = disc_on_disk(
        &dir.0.join("disc"),
        &[("alpha", invented_map(1)), ("beta", invented_map(2))],
    );
    let root = dir.0.join("store");
    prepare_run(&root, &a, Some(&disc)).unwrap();
    std::fs::remove_dir_all(dir.0.join("disc")).unwrap();
    let (p, done) = prepare_run(&root, &b, None).unwrap();
    assert_eq!(p.input, Input::Store);
    assert!(!p.plan.reset);
    assert_eq!(done, ["alpha", "beta"]);
    for w in &p.plan.maps {
        assert_eq!(w.why, extract::Why::Layouts);
        assert_eq!(w.groups, Some(BTreeSet::from([g(b"matg")])));
    }
    assert!(prepare_run(&root, &b, None).unwrap().0.plan.is_empty());
    for (stem, tweak) in [("alpha", 1u8), ("beta", 2)] {
        assert_eq!(reassembled(&root, stem), invented_map(tweak), "{stem}");
    }
}

/// An extractor version change empties the tree, and the whole store is
/// taken again from its own skeletons, with no disc. A run stopped after
/// the reset leaves the stash the next one reads.
#[test]
fn compile_re_extracts_everything_from_the_store_after_a_reset() {
    let dir = TempDir::new("compile-store-reset");
    let schema = Schema::parse(SCHEMA).unwrap();
    let disc = disc_on_disk(
        &dir.0.join("disc"),
        &[("alpha", invented_map(1)), ("beta", invented_map(2))],
    );
    let root = dir.0.join("store");
    prepare_run(&root, &schema, Some(&disc)).unwrap();
    std::fs::remove_dir_all(dir.0.join("disc")).unwrap();

    let mut repo = TagRepo::create(&root).unwrap();
    let mut rec = repo.extract_record().unwrap().unwrap();
    rec.extractor -= 1;
    repo.put_extract_record(&rec).unwrap();
    // A stopped run: stashed and reset, nothing extracted.
    compile::stash(&root, &["alpha".to_owned(), "beta".to_owned()]).unwrap();
    repo.reset(extract::EXTRACT_VERSION, format::VERSION)
        .unwrap();
    assert!(!repo.has_skeleton("alpha"));

    let (p, done) = prepare_run(&root, &schema, None).unwrap();
    assert_eq!(p.input, Input::Store);
    assert_eq!(
        plan_maps(&p.plan),
        [("alpha", extract::Why::New), ("beta", extract::Why::New)]
    );
    assert_eq!(done, ["alpha", "beta"]);
    assert!(
        !root.join(compile::STASH_DIR).exists(),
        "the stash is removed"
    );
    let repo = TagRepo::create(&root).unwrap();
    let recorded = repo.extract_record().unwrap().unwrap();
    for (stem, tweak) in [("alpha", 1u8), ("beta", 2)] {
        assert_eq!(reassembled(&root, stem), invented_map(tweak), "{stem}");
        assert!(repo.has_manifest(stem));
        assert_eq!(
            recorded.maps[stem].file,
            extract::file_hash(Cursor::new(invented_map(tweak))).unwrap()
        );
    }
    assert!(prepare_run(&root, &schema, None).unwrap().0.plan.is_empty());
}

/// A store with no skeleton for a map, and no disc, says it needs the disc
/// and changes nothing; an empty store with no disc says so too.
#[test]
fn compile_without_a_disc_names_what_needs_one() {
    let dir = TempDir::new("compile-needs-disc");
    let schema = Schema::parse(SCHEMA).unwrap();
    let root = dir.0.join("store");
    let e = prepare_run(&root, &schema, None).unwrap_err().to_string();
    assert!(e.contains("no disc"), "{e}");

    let disc = disc_on_disk(
        &dir.0.join("disc"),
        &[("alpha", invented_map(1)), ("beta", invented_map(2))],
    );
    prepare_run(&root, &schema, Some(&disc)).unwrap();
    std::fs::remove_file(root.join("maps").join("beta.extents")).unwrap();
    let before = snapshot(&root);
    let e = prepare_run(&root, &schema, None).unwrap_err().to_string();
    assert!(e.contains("beta") && e.contains("disc image"), "{e}");
    assert_eq!(snapshot(&root), before);
}

// --- Normal forms and the catalogue (docs/engine/asset-store.md, AS5) ---

/// An invented sound: the playback row it uses (by a marker byte), its
/// permutations as the chunk bytes of each, and whether a dialogue's extra
/// info goes with it.
struct Sound {
    playback: u8,
    permutations: Vec<Vec<Vec<u8>>>,
    dialogue: bool,
}

fn sample_a() -> Sound {
    Sound {
        playback: 0x21,
        permutations: vec![
            vec![b"first chunk of alpha".to_vec(), b"and its second".to_vec()],
            vec![b"alpha's other permutation".to_vec()],
        ],
        dialogue: false,
    }
}

fn sample_b() -> Sound {
    Sound {
        playback: 0x42,
        permutations: vec![vec![b"bravo's only permutation".to_vec()]],
        dialogue: true,
    }
}

/// The sound gestalt of an invented map that holds `sounds`, and their
/// `snd!` tags (`sound\<name>`): each is one pitch range, at the index its
/// place in `sounds` gives, and each permutation's chunks are its bytes.
fn invented_sounds(sounds: &[(&str, &Sound)]) -> (LooseTag, Vec<LooseTag>) {
    const PLAYBACK: usize = 0x38;
    const RANGE: usize = 0xc;
    const PERMUTATION: usize = 0x10;
    const CHUNK: usize = 0xc;
    const EXTRA: usize = 0x2c;
    const LOCATOR: usize = 0x10;
    let gestalt_key = TagKey::new(g(b"ugh!"), "sound\\gestalt");
    let mut playbacks = Vec::new();
    let mut ranges = Vec::new();
    let mut permutations = Vec::new();
    let mut chunks = Vec::new();
    let mut chunk_fields = Vec::new();
    let mut extra_infos = Vec::new();
    let mut extra_fields = Vec::new();
    let mut locators = Vec::new();
    let mut raws = Vec::new();
    let mut tags = Vec::new();
    let put = |bytes: &[u8], raws: &mut Vec<RawRes>| -> u32 {
        let i = raws.len() as u32;
        raws.push(RawRes {
            origin: Origin::Local,
            len: bytes.len() as u32,
            hash: sha256(bytes),
        });
        i
    };
    for (i, (name, s)) in sounds.iter().enumerate() {
        let mut row = vec![0u8; PLAYBACK];
        row[0] = s.playback;
        playbacks.extend(row);
        let first_permutation = (permutations.len() / PERMUTATION) as u16;
        for (j, permutation) in s.permutations.iter().enumerate() {
            let first_chunk = (chunks.len() / CHUNK) as u16;
            for sample in permutation {
                let at = chunks.len();
                chunks.extend([0; 4]);
                chunks.extend((sample.len() as u32).to_le_bytes());
                chunks.extend([0xff; 4]);
                let raw = put(sample, &mut raws);
                chunk_fields.push(((at / CHUNK) as u32 * CHUNK as u32, Value::Raw(raw)));
            }
            let mut row = vec![
                j as u8,
                0x11,
                0x22,
                0x33,
                0x44,
                if s.dialogue { j as u8 } else { 0xff },
            ];
            row.extend((100 * j as u16 + 7).to_le_bytes());
            let size: u32 = permutation.iter().map(|c| c.len() as u32).sum();
            row.extend(size.to_le_bytes());
            row.extend(first_chunk.to_le_bytes());
            row.extend((permutation.len() as u16).to_le_bytes());
            assert_eq!(row.len(), PERMUTATION);
            permutations.extend(row);
        }
        let mut row = vec![0xaa; 8];
        row.extend(first_permutation.to_le_bytes());
        row.extend((s.permutations.len() as u16).to_le_bytes());
        ranges.extend(row);
        let extra_info = if s.dialogue {
            let at = extra_infos.len();
            let base = (at / EXTRA) as u32 * EXTRA as u32;
            let mut row = vec![0u8; EXTRA];
            row[0] = s.playback;
            row[0xc..0x10].copy_from_slice(&0x40u32.to_le_bytes());
            row[0x10] = 16;
            let raw = put(&vec![s.playback; 0x40], &mut raws);
            extra_fields.push((base + 0x8, Value::Raw(raw)));
            for l in 0..2u8 {
                let mut locator = vec![0u8; LOCATOR];
                locator[0] = s.playback;
                locator[8..12].copy_from_slice(&8u32.to_le_bytes());
                locator[12..16].copy_from_slice(&(u32::from(l) * 8).to_le_bytes());
                locators.extend(locator);
            }
            extra_fields.push((base + 0x18, Value::Block(6)));
            extra_fields.push((base + 0x20, Value::Ref(gestalt_key.clone())));
            extra_infos.extend(row);
            at as u16
        } else {
            0xffff
        };
        let first = (ranges.len() / RANGE - 1) as u16;
        let mut bytes = Vec::new();
        bytes.extend(3u16.to_le_bytes());
        bytes.extend([2, 1, 0, 1]);
        bytes.extend((i as u16).to_le_bytes());
        bytes.extend(first.to_le_bytes());
        bytes.push(1);
        bytes.extend([9, 9, 9]);
        bytes.extend(extra_info.to_le_bytes());
        bytes.extend(250u32.to_le_bytes());
        assert_eq!(bytes.len(), 0x14);
        tags.push(LooseTag {
            key: TagKey::new(g(b"snd!"), format!("sound\\{name}")),
            body: Body::Tree(Tree {
                region: Region::Metadata,
                nodes: vec![Node {
                    bytes,
                    count: 1,
                    size: 0x14,
                    align: 4,
                    fields: Vec::new(),
                }],
                raws: Vec::new(),
                losses: Vec::new(),
            }),
        });
    }
    let node = |count: usize, size: usize, bytes: Vec<u8>, fields: Vec<(u32, Value)>| Node {
        bytes,
        count: count as u32,
        size: size as u32,
        align: 4,
        fields,
    };
    let mut root_fields = Vec::new();
    for (offset, at, len) in [
        (0x0u32, 1u32, playbacks.len()),
        (0x20, 2, ranges.len()),
        (0x28, 3, permutations.len()),
        (0x40, 4, chunks.len()),
        (0x50, 5, extra_infos.len()),
    ] {
        if len > 0 {
            root_fields.push((offset, Value::Block(at)));
        }
    }
    let gestalt = LooseTag {
        key: gestalt_key,
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![
                node(1, 0x58, vec![0; 0x58], root_fields),
                node(playbacks.len() / PLAYBACK, PLAYBACK, playbacks, Vec::new()),
                node(ranges.len() / RANGE, RANGE, ranges, Vec::new()),
                node(
                    permutations.len() / PERMUTATION,
                    PERMUTATION,
                    permutations,
                    Vec::new(),
                ),
                node(chunks.len() / CHUNK, CHUNK, chunks, chunk_fields),
                node(extra_infos.len() / EXTRA, EXTRA, extra_infos, extra_fields),
                node(locators.len() / LOCATOR, LOCATOR, locators, Vec::new()),
            ],
            raws,
            losses: Vec::new(),
        }),
    };
    (gestalt, tags)
}

/// The form of the `snd!` `tag` as read for the map whose gestalt is `gestalt`.
fn sound_form(tag: &LooseTag, gestalt: &LooseTag) -> LooseTag {
    let mut tables = normal::Tables {
        gestalt: Some(gestalt),
        ..normal::Tables::none()
    };
    normal::form(tag, &mut tables).unwrap()
}

/// Two maps hold one sound at two indices of their gestalts: one asset. A
/// third map's copy with another sample is another asset.
#[test]
fn a_sound_at_two_indices_of_two_gestalts_is_one_asset() {
    let (a, b) = (sample_a(), sample_b());
    let (gestalt_a, sounds_a) = invented_sounds(&[("alpha", &a), ("bravo", &b)]);
    let (gestalt_b, sounds_b) = invented_sounds(&[("bravo", &b), ("alpha", &a)]);
    // The same two sounds the other way round: other indices in each map.
    let header = |t: &LooseTag| t.tree().unwrap().nodes[0].bytes.clone();
    assert_ne!(header(&sounds_a[0]), header(&sounds_b[1]));
    assert_ne!(header(&sounds_a[1]), header(&sounds_b[0]));
    for (x, y) in [(&sounds_a[0], &sounds_b[1]), (&sounds_a[1], &sounds_b[0])] {
        let (fx, fy) = (sound_form(x, &gestalt_a), sound_form(y, &gestalt_b));
        assert_eq!(fx, fy);
        assert_eq!(normal::hash(&fx), normal::hash(&fy));
        // A form is a tag: it encodes and reads back the same.
        assert_eq!(decode(&encode(&fx)).unwrap(), fx);
    }

    let form = sound_form(&sounds_a[0], &gestalt_a);
    let nodes = &form.tree().unwrap().nodes;
    // The indices are out: the playback, the first pitch range.
    let root = &nodes[0];
    assert_eq!(root.size, 0x14 + 8 + 8);
    assert_eq!(&root.bytes[0x6..0xa], &[0, 0, 0, 0]);
    assert_eq!(&root.bytes[0xa], &1);
    assert_eq!(&root.bytes[0xe..0x10], &[0xff, 0xff]);
    assert_eq!(
        root.fields,
        vec![(0x14, Value::Block(1)), (0x1c, Value::Block(2))]
    );
    // The rows are in: the playback, the range (its first permutation out,
    // its count kept), each permutation (its first chunk out), each chunk.
    assert_eq!(nodes.len(), 8);
    assert_eq!((&nodes[1].bytes[..1], nodes[1].size), (&[0x21][..], 0x38));
    assert_eq!(nodes[2].size, 0xc + 8 * 2);
    assert_eq!(&nodes[2].bytes[0x8..0xc], &[0, 0, 2, 0]);
    assert_eq!(
        nodes[2].fields,
        vec![(0xc, Value::Block(3)), (0x14, Value::Block(6))]
    );
    assert_eq!(
        nodes[3].fields,
        vec![(0x10, Value::Block(4)), (0x18, Value::Block(5))]
    );
    assert_eq!(&nodes[3].bytes[0xc..0x10], &[0, 0, 2, 0]);
    assert_eq!(nodes[4].size, 0xc);
    assert_eq!(nodes[4].fields, vec![(0, Value::Raw(0))]);
    assert_eq!(form.tree().unwrap().raws.len(), 3);
    assert_eq!(canonical(&form).len(), 8);

    // A dialogue sound brings the extra info it names, without the datum
    // of the gestalt it came from.
    let bravo = sound_form(&sounds_a[1], &gestalt_a);
    let nodes = &bravo.tree().unwrap().nodes;
    assert_eq!(nodes[0].size, 0x14 + 8 + 8 + 8);
    assert_eq!(
        &nodes[0].bytes[0x14 + 8 + 8..0x14 + 8 + 8 + 4],
        &1u32.to_le_bytes()
    );
    // The extra info row, opened before the array its block names.
    let extra = &nodes[5];
    assert_eq!(extra.size, 0x2c);
    assert_eq!(
        extra.fields,
        vec![(0x8, Value::Raw(1)), (0x18, Value::Block(6))]
    );
    assert_eq!(nodes[6].count, 2);
    assert_eq!(&nodes[6].bytes[..1], &[0x42]);
    assert_eq!(bravo.tree().unwrap().raws.len(), 2);

    // Another sample is another asset.
    let other = Sound {
        permutations: vec![
            vec![
                b"first chunk of alpha".to_vec(),
                b"and its OTHER second".to_vec(),
            ],
            vec![b"alpha's other permutation".to_vec()],
        ],
        ..sample_a()
    };
    let (gestalt_c, sounds_c) = invented_sounds(&[("alpha", &other)]);
    assert_ne!(
        normal::hash(&form),
        normal::hash(&sound_form(&sounds_c[0], &gestalt_c))
    );
    // A map with no gestalt leaves its sounds as extracted.
    let as_is = normal::form(&sounds_a[0], &mut normal::Tables::none()).unwrap();
    assert_eq!(as_is, sounds_a[0]);
    // An index that names no row is an error naming the sound and the gestalt.
    let mut broken = sounds_a[0].clone();
    if let Body::Tree(t) = &mut broken.body {
        t.nodes[0].bytes[8..10].copy_from_slice(&7u16.to_le_bytes());
    }
    let e = normal::form(
        &broken,
        &mut normal::Tables {
            gestalt: Some(&gestalt_a),
            ..normal::Tables::none()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("sound\\alpha.snd!")
            && e.contains("sound\\gestalt.ugh!")
            && e.contains("pitch range 7"),
        "{e}"
    );
}

/// An invented bitmap of one image whose low-detail picture is `size` bytes
/// at `offset` of its map's region.
fn invented_bitmap(offset: u32, size: u32) -> LooseTag {
    let mut image = vec![0u8; 0x74];
    image[0..4].copy_from_slice(b"bitm");
    image[4..8].copy_from_slice(&16u16.to_le_bytes().repeat(2));
    image[0x60..0x64].copy_from_slice(&offset.to_le_bytes());
    image[0x64..0x68].copy_from_slice(&size.to_le_bytes());
    let node = |size: u32, bytes: Vec<u8>, fields: Vec<(u32, Value)>| Node {
        bytes,
        count: 1,
        size,
        align: 4,
        fields,
    };
    LooseTag {
        key: TagKey::new(g(b"bitm"), "objects\\picture"),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![
                node(0x4c, vec![0; 0x4c], vec![(0x44, Value::Block(1))]),
                node(0x74, image, Vec::new()),
            ],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

/// The same picture at two places of two maps' low-detail regions is one
/// asset; other pixels are another.
#[test]
fn a_bitmaps_low_detail_picture_is_hashed_not_placed() {
    let picture: Vec<u8> = (0..48u32).map(|i| (i * 5) as u8).collect();
    let region = |fill: u8, at: usize, picture: &[u8]| -> Vec<u8> {
        let mut r = vec![fill; 0x300];
        r[at..at + picture.len()].copy_from_slice(picture);
        r
    };
    let mut other = picture.clone();
    other[47] ^= 1;
    let (a, b, c) = (
        region(0xaa, 0x20, &picture),
        region(0xbb, 0x180, &picture),
        region(0xcc, 0x20, &other),
    );
    let form = |bitmap: &LooseTag, region: &[u8]| -> LooseTag {
        let mut tables = normal::Tables {
            low_detail: region,
            ..normal::Tables::none()
        };
        normal::form(bitmap, &mut tables).unwrap()
    };
    let (fa, fb, fc) = (
        form(&invented_bitmap(0x20, 48), &a),
        form(&invented_bitmap(0x180, 48), &b),
        form(&invented_bitmap(0x20, 48), &c),
    );
    assert_eq!(fa, fb);
    assert_eq!(normal::hash(&fa), normal::hash(&fb));
    assert_ne!(normal::hash(&fa), normal::hash(&fc));
    // The offset is out and the picture is a raw resource of the image.
    let images = &fa.tree().unwrap().nodes[1];
    assert_eq!(&images.bytes[0x60..0x64], &[0; 4]);
    assert_eq!(&images.bytes[0x64..0x68], &48u32.to_le_bytes());
    assert_eq!(images.fields, vec![(0x60, Value::Raw(0))]);
    assert_eq!(
        fa.tree().unwrap().raws,
        vec![RawRes {
            origin: Origin::Local,
            len: 48,
            hash: sha256(&picture),
        }]
    );
    assert_eq!(decode(&encode(&fa)).unwrap(), fa);
    // With no region, or a picture the region does not hold, the place
    // stays as extracted.
    let place = |t: &LooseTag| t.tree().unwrap().nodes[1].bytes[0x60..0x64].to_vec();
    let as_is = normal::form(&invented_bitmap(0x20, 48), &mut normal::Tables::none()).unwrap();
    assert_eq!(place(&as_is), 0x20u32.to_le_bytes());
    assert!(as_is.tree().unwrap().nodes[1].fields.is_empty());
    let short = form(&invented_bitmap(0x2f0, 48), &a);
    assert_eq!(place(&short), 0x2f0u32.to_le_bytes());
    assert!(short.tree().unwrap().raws.is_empty());
}

/// A raw resource of the globals or the scenario that was extracted with
/// the padding after it is cut to the size its element stores. A locale's,
/// another group's, or one with no layout to say so is left as extracted.
#[test]
fn a_padded_raw_of_the_globals_is_cut_to_its_stored_size() {
    let schema = Schema::parse(
        "group matg 0x1a4 align 4\n  0x0 raw\n  0x4 size 0x0\n  0x19c raw\n  0x1a0 size 0x19c\n\
         group weap 0x8 align 4\n  0x0 raw\n  0x4 size 0x0\n",
    )
    .unwrap();
    // A tag of `group` with a root of `root` bytes holding one raw
    // resource, its size stored at `size_at`.
    let tag = |group: &[u8; 4],
               path: &str,
               root: usize,
               field: usize,
               size_at: usize,
               stored: u32,
               bytes: Vec<u8>| {
        let mut b = vec![0u8; root];
        b[size_at..size_at + 4].copy_from_slice(&stored.to_le_bytes());
        LooseTag {
            key: TagKey::new(g(group), path),
            body: Body::Tree(Tree {
                region: Region::Metadata,
                nodes: vec![Node {
                    bytes: b,
                    count: 1,
                    size: root as u32,
                    align: 4,
                    fields: vec![(field as u32, Value::Raw(0))],
                }],
                raws: vec![RawRes {
                    origin: Origin::Local,
                    len: bytes.len() as u32,
                    hash: sha256(&bytes),
                }],
                losses: Vec::new(),
            }),
        }
    };
    let data: Vec<u8> = (0..100u32).map(|i| (i * 3) as u8).collect();
    let padded = |fill: u8, n: usize| -> Vec<u8> {
        let mut b = data.clone();
        b.extend(std::iter::repeat(fill).take(n));
        b
    };
    // Both maps' files, by hash.
    let mut files: HashMap<[u8; 32], Vec<u8>> = HashMap::new();
    for bytes in [padded(0, 200), padded(0xcd, 4)] {
        files.insert(sha256(&bytes), bytes);
    }
    let mut read = |hash: &[u8; 32]| -> Result<Vec<u8>> { Ok(files[hash].clone()) };
    let mut tables = normal::Tables {
        raw: Some(&mut read),
        schema: Some(&schema),
        ..normal::Tables::none()
    };
    let globals = |bytes: Vec<u8>| tag(b"matg", "globals\\globals", 0x1a4, 0x0, 0x4, 100, bytes);
    let (a, b) = (
        normal::form(&globals(padded(0, 200)), &mut tables).unwrap(),
        normal::form(&globals(padded(0xcd, 4)), &mut tables).unwrap(),
    );
    assert_eq!(a, b);
    assert_eq!(
        a.tree().unwrap().raws,
        vec![RawRes {
            origin: Origin::Local,
            len: 100,
            hash: sha256(&data),
        }]
    );
    assert_eq!(decode(&encode(&a)).unwrap(), a);
    // A size of zero, or one past the resource, cuts nothing.
    let whole = |stored: u32| {
        tag(
            b"matg",
            "globals\\globals",
            0x1a4,
            0x0,
            0x4,
            stored,
            padded(0, 200),
        )
    };
    for stored in [0, 300, 400] {
        assert_eq!(
            normal::form(&whole(stored), &mut tables)
                .unwrap()
                .tree()
                .unwrap()
                .raws[0]
                .len,
            300
        );
    }
    // A locale's resource keeps the length its descriptor gives it.
    let locale = |bytes: Vec<u8>| {
        tag(
            b"matg",
            "globals\\globals",
            0x1a4,
            0x19c,
            0x1a0,
            50,
            bytes,
        )
    };
    assert_ne!(
        normal::form(&locale(padded(0, 200)), &mut tables).unwrap(),
        normal::form(&locale(padded(0xcd, 4)), &mut tables).unwrap()
    );
    // Another group's sizes the extractor trusts: nothing is cut.
    let weapon = |bytes: Vec<u8>| tag(b"weap", "objects\\gun", 0x8, 0x0, 0x4, 100, bytes);
    assert_ne!(
        normal::form(&weapon(padded(0, 200)), &mut tables).unwrap(),
        normal::form(&weapon(padded(0xcd, 4)), &mut tables).unwrap()
    );
    // With no store to read the bytes from, or no layout, nothing is cut.
    let mut tables = normal::Tables {
        schema: Some(&schema),
        ..normal::Tables::none()
    };
    assert_ne!(
        normal::form(&globals(padded(0, 200)), &mut tables).unwrap(),
        normal::form(&globals(padded(0xcd, 4)), &mut tables).unwrap()
    );
}

/// An invented store of three maps. `alpha` and `beta` hold the same two
/// sounds the other way round, `gamma` the first with another sample; each
/// has a globals tag of its own and a weapon that names `sound\alpha`.
fn sound_store(dir: &std::path::Path) {
    let mut repo = TagRepo::create(dir).unwrap();
    let (a, b) = (sample_a(), sample_b());
    let other = Sound {
        permutations: vec![
            vec![
                b"first chunk of alpha".to_vec(),
                b"and its OTHER second".to_vec(),
            ],
            vec![b"alpha's other permutation".to_vec()],
        ],
        ..sample_a()
    };
    let maps: [(&str, Vec<(&str, &Sound)>); 3] = [
        ("alpha", vec![("alpha", &a), ("bravo", &b)]),
        ("beta", vec![("bravo", &b), ("alpha", &a)]),
        ("gamma", vec![("alpha", &other), ("bravo", &b)]),
    ];
    for (i, (stem, sounds)) in maps.iter().enumerate() {
        let (gestalt, sounds) = invented_sounds(sounds);
        let mut globals = sample().tags[0].clone();
        if let Body::Tree(t) = &mut globals.body {
            t.nodes[0].bytes[12] = 0x10 + i as u8;
        }
        let gun = LooseTag {
            key: TagKey::new(g(b"weap"), "objects\\gun"),
            body: Body::Tree(Tree {
                region: Region::Metadata,
                nodes: vec![Node {
                    bytes: vec![0; 4],
                    count: 1,
                    size: 4,
                    align: 4,
                    fields: vec![(0, Value::Ref(TagKey::new(g(b"snd!"), "sound\\alpha")))],
                }],
                raws: Vec::new(),
                losses: Vec::new(),
            }),
        };
        let mut m = store::Manifest {
            map: (*stem).to_owned(),
            ..Default::default()
        };
        for tag in [globals, gestalt, gun].into_iter().chain(sounds) {
            let variant = match repo.put(&tag).unwrap() {
                store::Stored::New | store::Stored::Same => None,
                store::Stored::Variant(v) => Some(v),
            };
            m.tags.push((tag.key.clone(), variant));
        }
        repo.put_manifest(&m, &[0; 0x800]).unwrap();
    }
}

/// The catalogue lists each asset by hash with the maps reading it and the
/// keys that reference it; one rebuilt from the manifests equals the one
/// kept, and a changed manifest makes it stale.
#[test]
fn the_catalogue_lists_each_asset_by_hash_and_is_rebuilt_to_the_same() {
    let dir = TempDir::new("catalogue");
    sound_store(&dir.0);
    let schema = Schema::parse(SCHEMA).unwrap();
    let (c, report) = catalogue::build(&dir.0, &schema).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.maps, 3);
    let key = |group: &[u8; 4], path: &str| TagKey::new(g(group), path);
    fn names(a: &catalogue::Asset) -> Vec<&str> {
        a.maps.iter().map(String::as_str).collect()
    }
    // One sound at two indices in two maps: one asset, read from two files.
    let alpha = c.of_key(&key(b"snd!", "sound\\alpha"));
    assert_eq!(alpha.len(), 2, "{alpha:?}");
    let shared = alpha.iter().find(|a| a.maps.len() == 2).unwrap();
    assert_eq!(names(shared), ["alpha", "beta"]);
    assert_eq!(shared.files.len(), 2);
    assert_eq!(
        c.hash_in("alpha", &key(b"snd!", "sound\\alpha")),
        c.hash_in("beta", &key(b"snd!", "sound\\alpha"))
    );
    assert_ne!(
        c.hash_in("alpha", &key(b"snd!", "sound\\alpha")),
        c.hash_in("gamma", &key(b"snd!", "sound\\alpha"))
    );
    // The other is one asset in all three maps.
    let bravo = c.of_key(&key(b"snd!", "sound\\bravo"));
    assert_eq!(bravo.len(), 1);
    assert_eq!(names(&bravo[0]), ["alpha", "beta", "gamma"]);
    assert!(bravo[0].hash.len() == 64);
    // Before the normal forms: the files. After: the assets.
    let variants = |group: &[u8; 4], m: &BTreeMap<FourCC, catalogue::Variants>| m[&g(group)];
    assert_eq!(
        variants(b"snd!", &report.before),
        // A sound's file is its header alone: gamma's copy of alpha's is
        // the main file, though its map plays other samples.
        catalogue::Variants {
            keys: 2,
            contents: 4,
            split: 2,
            instances: 6,
            others: 2
        }
    );
    assert_eq!(
        variants(b"snd!", &report.after),
        catalogue::Variants {
            keys: 2,
            contents: 3,
            split: 1,
            instances: 6,
            others: 1
        }
    );
    assert_eq!(
        variants(b"weap", &report.before),
        catalogue::Variants {
            keys: 1,
            contents: 1,
            split: 0,
            instances: 3,
            others: 0
        }
    );
    let (before, after) = report.totals();
    assert_eq!(
        (before.contents, before.split, after.contents, after.split),
        (11, 4, 10, 3)
    );
    // The weapon names alpha's sound.
    assert_eq!(
        c.users(&key(b"snd!", "sound\\alpha")).unwrap(),
        &BTreeSet::from([key(b"weap", "objects\\gun")])
    );
    assert!(c.users(&key(b"weap", "objects\\gun")).is_none());
    // A map's assets, each with its hash, sorted by key.
    let of_alpha = c.of_map("alpha");
    assert_eq!(of_alpha.len(), 5);
    assert!(of_alpha.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(c.of_map("nowhere").len(), 0);

    // Written and read back: the same catalogue, and current.
    catalogue::write(&dir.0, &c).unwrap();
    let back = catalogue::read(&dir.0).unwrap().unwrap();
    assert_eq!(back, c);
    assert_eq!(back.to_text(), c.to_text());
    assert!(back.is_current(&dir.0).unwrap());
    assert_eq!(
        TagRepo::open(&dir.0).unwrap().catalogue().unwrap(),
        Some(back.clone())
    );
    assert_eq!(catalogue::update(&dir.0, &schema, false).unwrap(), None);
    // Rebuilt from the manifests, it is the one kept.
    let (again, _) = catalogue::build(&dir.0, &schema).unwrap();
    assert_eq!(again, c);
    assert_eq!(
        catalogue::update(&dir.0, &schema, true)
            .unwrap()
            .map(|b| b.maps),
        Some(3)
    );
    // A manifest that changes makes it stale, and the next run rebuilds it.
    let repo = TagRepo::open(&dir.0).unwrap();
    let mut m = repo.manifest("beta").unwrap();
    m.name = "renamed".into();
    repo.write_manifest(&m).unwrap();
    assert!(!back.is_current(&dir.0).unwrap());
    assert!(catalogue::update(&dir.0, &schema, false).unwrap().is_some());
    let text = std::fs::read_to_string(dir.0.join("maps").join("beta.txt")).unwrap();
    assert_eq!(
        catalogue::read(&dir.0).unwrap().unwrap().maps["beta"],
        hex(&sha256(text.as_bytes()))
    );
    // A file that does not read is a line in the report, not a failure.
    let junk = TagKey::new(g(b"jpt!"), "invented\\broken");
    std::fs::create_dir_all(repo.path_of(&junk, None).parent().unwrap()).unwrap();
    std::fs::write(repo.path_of(&junk, None), b"not a tag file").unwrap();
    for stem in ["alpha", "beta", "gamma"] {
        let mut m = repo.manifest(stem).unwrap();
        m.tags.push((junk.clone(), None));
        repo.write_manifest(&m).unwrap();
    }
    let (c, report) = catalogue::build(&dir.0, &schema).unwrap();
    assert_eq!(report.problems.len(), 3, "{:?}", report.problems);
    assert!(
        report.problems[0].contains("alpha")
            && report.problems[0].contains("invented\\broken.jpt!"),
        "{}",
        report.problems[0]
    );
    assert!(c.of_key(&junk).is_empty());
}

/// `prepare` writes the catalogue with the store, leaves a current one
/// alone, writes a deleted one again, and a reset takes it.
#[test]
fn prepare_writes_the_catalogue_and_leaves_a_current_one_alone() {
    let dir = TempDir::new("compile-catalogue");
    let schema = Schema::parse(SCHEMA).unwrap();
    let disc = disc_on_disk(
        &dir.0.join("disc"),
        &[("alpha", invented_map(1)), ("beta", invented_map(2))],
    );
    let root = dir.0.join("store");
    let (p, _) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    let report = p.catalogue.as_ref().expect("the catalogue is written");
    assert_eq!(report.maps, 2);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let c = catalogue::read(&root).unwrap().unwrap();
    assert_eq!(c.normal, normal::VERSION);
    assert!(c.is_current(&root).unwrap());
    // Each map: its globals, the bitmap, its alias and the scenario.
    assert_eq!(c.of_map("alpha").len(), 4);
    assert_eq!(
        c.of_key(&TagKey::new(g(b"matg"), "globals\\globals")).len(),
        2
    );
    assert_eq!(c.of_key(&TagKey::new(g(b"bitm"), "a\\pixels")).len(), 1);
    assert_eq!(
        c.users(&TagKey::new(g(b"bitm"), "a\\pixels")).unwrap(),
        &BTreeSet::from([TagKey::new(g(b"matg"), "globals\\globals")])
    );
    // Nothing stale: the catalogue is not written again.
    let before = snapshot(&root);
    let (p, _) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert!(p.catalogue.is_none());
    assert_eq!(snapshot(&root), before);
    // Deleted, it is written again by the next run.
    std::fs::remove_file(root.join(catalogue::CATALOGUE_FILE)).unwrap();
    let (p, _) = prepare_run(&root, &schema, Some(&disc)).unwrap();
    assert!(p.catalogue.is_some());
    assert_eq!(catalogue::read(&root).unwrap().unwrap(), c);
    // A reset takes the catalogue with the rest.
    TagRepo::create(&root)
        .unwrap()
        .reset(extract::EXTRACT_VERSION, format::VERSION)
        .unwrap();
    assert!(!root.join(catalogue::CATALOGUE_FILE).exists());
}
