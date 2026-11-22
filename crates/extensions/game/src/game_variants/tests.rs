//! The store, porting, the merged list and the Live refresh, on invented
//! files, fake guest memory and fake Live answers. The bundles are real
//! format 2 files (`variants::portable`) with compiled scripts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use content::Sha256;
use variants::ext::{Mode, Script};
use variants::portable::Bundle;
use variants::save::{RUNTIME_KEY, SaveStore, encode_file, encode_meta};
use variants::{GameType, GameVariant, SettingId};

use super::inject;
use super::port::{self, PortScript};
use super::store::{Origin, Store};
use crate::community_variants::{self as cv, Group, Mem, NewEntry};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("h2-game-variants-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn prefix(h: &Sha256) -> [u8; 8] {
    let mut p = [0; 8];
    p.copy_from_slice(&h.0[..8]);
    p
}

/// A Slayer script of its own, scoring `k` a kill.
fn ir(k: u32) -> Vec<u8> {
    let src = format!(
        "rules \"Score {k}\" 1\ncarrier slayer\n\non kill(victim, killer) {{ score(killer, {k}) }}\n"
    );
    ::rules::compile(&src).unwrap().encode()
}

/// A Slayer record named `name` with score `score`.
fn record(name: &str, score: i32) -> GameVariant {
    let mut v = GameVariant::blank(GameType::Slayer);
    v.set_name(name).unwrap();
    v.set(SettingId(2), score).unwrap();
    v
}

/// `record(name, score)` as a bundle, carrying `blob` when given.
fn bundle(name: &str, score: i32, blob: Option<Vec<u8>>) -> Bundle {
    let script = blob.map_or(PortScript::Keep, PortScript::Ir);
    variants::portable::port(&record(name, score), None, Some("Made by a player"), script).unwrap()
}

/// `v` with its block naming script `h`.
fn naming(mut v: GameVariant, h: &Sha256) -> GameVariant {
    let mut e = v.ext();
    e.script = Script {
        mode: Mode::Rules,
        prefix: prefix(h),
        options: 0,
    };
    v.set_ext(&e).unwrap();
    v
}

#[test]
fn the_store_reads_its_folders_and_indexes_scripts_by_prefix() {
    let root = tmp("open");
    let (ir_a, ir_b) = (ir(1), ir(2));
    let (ha, hb) = (Sha256::of(&ir_a), Sha256::of(&ir_b));
    let a = bundle("Imported", 10, Some(ir_a.clone()));
    std::fs::write(root.join("a.h2variant"), a.canonical()).unwrap();
    std::fs::write(root.join("junk.h2variant"), "not json").unwrap();
    std::fs::create_dir_all(root.join("live")).unwrap();
    // The same file again: one entry.
    std::fs::write(root.join("live/v2.h2variant"), a.canonical()).unwrap();
    // A Live variant waiting for its IR, which is in scripts/ already: it
    // becomes its bundle at open.
    let pending = live("v1", "From Live", Some(ir_b.as_slice()));
    std::fs::write(
        root.join("live/v1.pending.json"),
        serde_json::to_string(&pending).unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join(format!("scripts/{hb}.ir")), &ir_b).unwrap();
    std::fs::write(root.join(format!("scripts/{ha}.ir")), b"tampered").unwrap();

    let (s, lines) = Store::open(&root);
    assert_eq!(s.entries().len(), 2, "{lines:?}");
    assert_eq!(s.entries()[0].origin, Origin::Imported);
    assert_eq!(s.entries()[1].origin, Origin::Live);
    assert!(s.pending().is_empty());
    assert!(root.join("live/v1.h2variant").is_file());
    assert!(!root.join("live/v1.pending.json").exists());
    assert!(lines.iter().any(|l| l.contains("junk.h2variant")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("not its name")), "{lines:?}");
    assert_eq!(s.script(prefix(&ha)), Some(ir_a.as_slice()));
    assert_eq!(s.script(prefix(&hb)), Some(ir_b.as_slice()));
    assert_eq!(s.entries()[1].hash, Some(hb));
    assert_eq!(s.script([0; 8]), None);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_live_variant_without_its_ir_waits_with_its_full_hash() {
    let blob = ir(3);
    let h = Sha256::of(&blob);
    let mut s = Store::in_memory();
    assert_eq!(s.put_live(&live("v9", "Waiting", Some(blob.as_slice()))), Ok(false));
    assert!(s.entries().is_empty(), "no bundle without the IR");
    assert_eq!(s.pending().len(), 1);
    assert_eq!(s.full_hash(prefix(&h)), Some(h));
    assert_eq!(s.script(prefix(&h)), None);
    s.save_script(blob.clone()).unwrap();
    assert_eq!(s.put_live(&live("v9", "Waiting", Some(blob.as_slice()))), Ok(true));
    assert!(s.pending().is_empty());
    assert_eq!(s.entries()[0].hash, Some(h));
}

/// A save directory as the game writes one, holding `v`.
fn write_save(saves: &std::path::Path, dir: &str, v: &GameVariant) -> PathBuf {
    let d = saves.join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("SaveMeta.xbx"), encode_meta(&format!("Slayer: {}", v.name()))).unwrap();
    let file = d.join("Slayer");
    std::fs::write(&file, encode_file(v, &RUNTIME_KEY)).unwrap();
    file
}

#[test]
fn saved_variants_port_once_and_the_saves_are_left_alone() {
    let root = tmp("port");
    let saves_root = root.join("UDATA");
    let store_root = root.join("game-variants");
    let a = write_save(&saves_root, "AAAA", &record("Fast", 25));
    let b = write_save(&saves_root, "BBBB", &record("Faster", 50));
    // A record naming a script whose IR the save does not have.
    let scripted = naming(record("Scripted", 5), &Sha256::of(b"x"));
    write_save(&saves_root, "CCCC", &scripted);
    let before = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let saves = SaveStore::new(&saves_root, None);

    let (mut s, _) = Store::open(&store_root);
    let report = port::port_saves(&mut s, &saves, &PortScript::Keep);
    assert_eq!(report.ported, 2, "{report:?}");
    assert_eq!(report.skipped, 0);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(report.refused[0].0, "Scripted");
    assert!(report.line().contains("ported 2"), "{}", report.line());
    assert_eq!(s.entries().len(), 2);
    assert!(s.entries().iter().all(|e| e.origin == Origin::Ported));
    // The sources are only read.
    assert_eq!((std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()), before);

    // The next boot: the bundles are on disk, nothing is ported again.
    let (mut again, _) = Store::open(&store_root);
    assert_eq!(again.entries().len(), 2);
    let report = port::port_saves(&mut again, &saves, &PortScript::Keep);
    assert_eq!((report.ported, report.skipped), (0, 2), "{report:?}");

    // A save edited since is a new source.
    write_save(&saves_root, "AAAA", &record("Fast", 30));
    let report = port::port_saves(&mut again, &saves, &PortScript::Keep);
    assert_eq!((report.ported, report.skipped), (1, 1), "{report:?}");

    // `builtin` attaches the engine's built-in script.
    let (mut fresh, _) = Store::open(&root.join("other"));
    let report = port::port_saves(&mut fresh, &saves, &PortScript::Builtin);
    assert!(report.ported >= 2, "{report:?}");
    let slayer = crate::rules::runtime::builtins()
        .iter()
        .find(|b| b.engine == 2)
        .unwrap();
    assert!(fresh.entries().iter().all(|e| e.hash == Some(slayer.id)));
    let _ = std::fs::remove_dir_all(&root);
}

/// Sparse little-endian memory; unwritten bytes read as missing.
#[derive(Default)]
struct Fake(HashMap<u32, u8>);
impl Mem for Fake {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.0.get(&(addr + i as u32)) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

const GOOF: u32 = 0x8000_0000;
const OLD: u32 = 0x8000_1000;
/// `goof +0x28`: the built-ins' count and address.
const BUILTINS: u32 = 0x28;
const ENTRIES: u32 = cv::DATA_BASE + 0x2000;
const OPTIONS: u32 = 0x8100_0000;

/// A goof tag with `n` invented retail entries.
fn world(n: u32) -> Fake {
    let mut m = Fake::default();
    m.wr(GOOF, &[0; 0x100]);
    for i in 0..n {
        let e: Vec<u8> = [0x0100_0000 + i, i % 7, 0, 0, i]
            .iter()
            .flat_map(|w: &u32| w.to_le_bytes())
            .collect();
        m.wr(OLD + i * 0x14, &e);
    }
    m.set_u32(GOOF + BUILTINS, n);
    m.set_u32(GOOF + BUILTINS + 4, OLD);
    m
}

fn store_of(names: &[&str]) -> Store {
    let mut s = Store::in_memory();
    for (k, name) in names.iter().enumerate() {
        s.add(Origin::Live, bundle(name, 10, Some(ir(k as u32 + 1)))).unwrap();
    }
    s
}

/// The listing is process-wide, so everything that reads it is here.
#[test]
fn the_merged_block_holds_retail_community_and_store_entries() {
    let community: Vec<NewEntry> = (0..cv::VARIANTS.len()).map(NewEntry::community).collect();
    let offered: HashMap<u32, Vec<i32>> = [(2, vec![10, 25])].into_iter().collect();

    // A value the editor does not offer leaves the variant out, by name.
    let mut s = store_of(&["Tag", "Hunt"]);
    s.add(Origin::Imported, bundle("Odd", 7, None)).unwrap();
    let (stored, lines) = inject::entries(&s, &offered, 101);
    assert_eq!(stored.len(), 2);
    assert!(lines.iter().any(|l| l.contains("Odd") && l.contains("does not offer")), "{lines:?}");
    assert_eq!(stored[0].name_id, inject::name_id(0));
    assert_eq!(stored[1].name_id, inject::name_id(1));
    assert_eq!(stored[0].pairs, [(2, 10)]);
    assert_eq!(stored[0].list_type, 0, "Slayer is list type 0");
    assert_eq!(stored[0].description, inject::DESCRIPTION_BASE);

    let mut m = world(39);
    let groups = [
        Group {
            entries: &community,
            start: cv::DATA_BASE,
            end: ENTRIES,
        },
        Group {
            entries: &stored,
            start: super::PAIRS_BASE,
            end: super::PAIRS_BASE + super::PAIRS_SIZE,
        },
    ];
    let merged = cv::extend_with(&mut m, GOOF, &groups, ENTRIES).unwrap();
    assert_eq!(merged.added, [cv::VARIANTS.len(), 2]);
    assert_eq!(merged.over, 0);
    let n = m.u32(GOOF + BUILTINS).unwrap() as usize;
    assert_eq!(n, 39 + cv::VARIANTS.len() + 2);
    assert_eq!(m.u32(GOOF + BUILTINS + 4), Some(ENTRIES));
    let at = |i: usize| ENTRIES + i as u32 * 0x14;
    assert_eq!(m.u32(at(0)), Some(0x0100_0000), "retail first, unchanged");
    assert_eq!(m.u32(at(39)), Some(cv::name_id(0)), "then the community's");
    let first = at(39 + cv::VARIANTS.len());
    assert_eq!(m.u32(first), Some(inject::name_id(0)), "then the store's");
    assert_eq!(m.u32(first + 12), Some(super::PAIRS_BASE));
    assert_eq!(m.u32(super::PAIRS_BASE), Some(2));
    assert_eq!(m.u32(super::PAIRS_BASE + 4), Some(10));
    // Again: already merged.
    assert_eq!(cv::extend_with(&mut m, GOOF, &groups, ENTRIES), Some(cv::Merged::default()));

    // The strings and the description numbers.
    assert_eq!(inject::text(inject::name_id(0)).as_deref(), Some("Tag \u{e007}"));
    assert_eq!(inject::text(inject::description_id(1)).as_deref(), Some("Made by a player"));
    assert_eq!(inject::text(inject::name_id(5)), None);
    assert_eq!(inject::description_for(inject::DESCRIPTION_BASE + 1), Some(inject::description_id(1)));
    assert_eq!(inject::description_for(93), None, "the community's last");
    assert!(inject::text(inject::description_for(100).unwrap()).is_some());
    assert_eq!(cv::description_answer(0x002c_a1fd, 95), Some(inject::description_id(1)));

    // At the start: a variant built from the "Tag" entry gets its block.
    let mut built = GameVariant::blank(GameType::Slayer);
    built.set_name("Tag \u{e007}").unwrap();
    built.set(SettingId(2), 10).unwrap();
    let rec = OPTIONS + crate::variant_ext::OPTIONS_VARIANT;
    m.wr(rec, built.bytes());
    let line = inject::apply_block(&mut m, OPTIONS).expect("ours");
    assert!(line.contains("Tag"), "{line}");
    let mut block = [0u8; variants::ext::LEN];
    assert!(m.rd(rec + variants::ext::AT as u32, &mut block));
    let want: [u8; variants::ext::LEN] = s.entries()[0].record
        [variants::ext::AT..variants::ext::AT + variants::ext::LEN]
        .try_into()
        .unwrap();
    assert_eq!(block, want);
    assert_eq!(variants::ext::Ext::in_effect(&block).script.mode, Mode::Rules);
    assert_eq!(inject::apply_block(&mut m, OPTIONS), None, "already there");
    // Another variant of the same name on another engine is not ours.
    let mut other = built.clone();
    other.set_game_type(GameType::King);
    m.wr(rec, other.bytes());
    assert_eq!(inject::apply_block(&mut m, OPTIONS), None);

    // 101 built-ins at most: the store's past it are left out.
    let names: Vec<String> = (0..60).map(|i| format!("V{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let many = store_of(&names);
    let (stored, _) = inject::entries(&many, &offered, 101);
    assert_eq!(stored.len(), 60);
    let mut m = world(39);
    let groups = [
        Group {
            entries: &community,
            start: cv::DATA_BASE,
            end: ENTRIES,
        },
        Group {
            entries: &stored,
            start: super::PAIRS_BASE,
            end: super::PAIRS_BASE + super::PAIRS_SIZE,
        },
    ];
    let merged = cv::extend_with(&mut m, GOOF, &groups, ENTRIES).unwrap();
    assert_eq!(merged.added, [cv::VARIANTS.len(), 101 - 39 - cv::VARIANTS.len()]);
    assert_eq!(merged.over, 60 - (101 - 39 - cv::VARIANTS.len()));
    assert_eq!(m.u32(GOOF + BUILTINS), Some(101));
}

#[test]
fn names_fit_the_record_and_end_with_the_icon() {
    let long = inject::display_name("A very long game variant name indeed, too long");
    assert!(long.ends_with(" \u{e007}"));
    assert!(long.encode_utf16().count() <= variants::record::MAX_NAME_UNITS);
    let field = inject::name_field("Tag \u{e007}");
    assert_eq!(&field[..2], &[b'T', 0]);
    assert_eq!(&field[8..10], &0xe007u16.to_le_bytes());
    assert!(field[10..].iter().all(|b| *b == 0));
}

/// A Live variant named `name`, naming the script `ir` hashes to.
fn live(id: &str, name: &str, ir: Option<&[u8]>) -> live_hle::game_variants::LiveVariant {
    let h = ir.map(Sha256::of);
    let v = match &h {
        Some(h) => naming(record(name, 10), h),
        None => record(name, 10),
    };
    live_hle::game_variants::LiveVariant {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        author: live_hle::game_variants::Author {
            xuid: "9000000000000001".into(),
            gamertag: "Alex".into(),
        },
        engine: 2,
        game_type: "slayer".into(),
        record_hex: super::bundle::hex(v.bytes()),
        script_hash: h.map(|h| h.to_string()),
        script_name: h.map(|_| "Script".into()),
        visibility: "public".into(),
        created: String::new(),
    }
}

#[test]
fn the_live_refresh_saves_each_variant_with_its_script() {
    use live_hle::game_variants::FetchError;
    let root = tmp("live");
    let blob = ir(4);
    let (s, _) = Store::open(&root);
    let s = Mutex::new(s);
    let served = blob.clone();
    let listed = vec![live("v1", "Tag", Some(blob.as_slice())), live("v2", "Plain", None)];
    super::refresh_from(&s, move |_| Ok(listed.clone()), move |_, _| Ok(served.clone()));
    let h = Sha256::of(&blob);
    {
        let s = s.lock().unwrap();
        assert_eq!(s.entries().len(), 2);
        assert_eq!(s.script(prefix(&h)), Some(blob.as_slice()));
    }
    // Saved as whole format 2 files: the next boot has them offline.
    let text = std::fs::read_to_string(root.join("live/v1.h2variant")).unwrap();
    let b = variants::portable::read(&text).unwrap();
    assert_eq!(b.ir().unwrap().as_deref(), Some(blob.as_slice()));
    let (again, _) = Store::open(&root);
    assert_eq!(again.entries().len(), 2);
    assert_eq!(again.script(prefix(&h)), Some(blob.as_slice()));

    // A fetch that fails leaves the variant pending, with its full hash.
    let other = ir(5);
    let ho = Sha256::of(&other);
    let listed = vec![live("v3", "Later", Some(other.as_slice()))];
    super::refresh_from(&s, move |_| Ok(listed.clone()), |_, _| Err(FetchError::NotFound));
    {
        let s = s.lock().unwrap();
        assert_eq!(s.entries().len(), 2);
        assert_eq!(s.pending().len(), 1);
        assert_eq!(s.full_hash(prefix(&ho)), Some(ho));
    }
    assert!(root.join("live/v3.pending.json").is_file());

    // Offline: the store keeps what it had.
    super::refresh_from(&s, |_| Err(FetchError::Offline), |_, _| Err(FetchError::NotFound));
    assert_eq!(s.lock().unwrap().entries().len(), 2);
    let _ = std::fs::remove_dir_all(&root);
}
