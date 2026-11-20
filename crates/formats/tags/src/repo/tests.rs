use super::format::{decode, encode};
use super::*;
use crate::schema::Schema;
use map::{CacheType, FourCC, Map};
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
