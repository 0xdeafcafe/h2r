//! Raw writes against synthetic guest memory and an in-memory file view:
//! a structure BSP with two clusters (one paged in, one not), a bitmap
//! with one image, and a sound whose permutation has two sample chunks in
//! the map's sound gestalt. No game data.

use super::*;
use runtime::guest::Permissions;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const META: u32 = 0x1000_0000;

const SCNR: u32 = 0xe000_0000;
const SBSP: u32 = 0xe111_0001;
const BITM: u32 = 0xe222_0002;

const SBSP_AT: u32 = META + 0x1000;
const CLUSTERS_AT: u32 = META + 0x2000;
const BITM_AT: u32 = META + 0x3000;
const IMAGES_AT: u32 = META + 0x4000;
/// Cluster 0's resident buffer (block offset 0).
const RESIDENT: u32 = META + 0x8000;

/// Cluster 0's raw block in the local file, cluster 1's, image 0's LOD 0
/// in shared.
const BLOCK0: u32 = 0x100;
const BLOCK1: u32 = 0x200;
const PIXELS: u32 = 0x40;
const BLOCK_SIZE: u32 = 0x40;

/// The file view: each file's bytes, and an overlay of single bytes.
#[derive(Default)]
struct Files {
    own: BTreeMap<u8, Vec<u8>>,
    overlay: BTreeMap<(u8, u64), u8>,
    appended: Vec<(u8, u64, u64)>,
    refusal: Option<String>,
}

fn key(f: ResourceFile) -> u8 {
    match f {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}

impl Files {
    fn view(&self, f: ResourceFile, at: u64, len: usize) -> Vec<u8> {
        let own = self.own.get(&key(f)).cloned().unwrap_or_default();
        (0..len as u64)
            .map(|i| {
                let p = at + i;
                self.overlay
                    .get(&(key(f), p))
                    .copied()
                    .unwrap_or_else(|| own.get(p as usize).copied().unwrap_or(0))
            })
            .collect()
    }
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Files>>);

impl RawFiles for Fake {
    fn ready(&mut self, _: &Guest) -> std::result::Result<(), String> {
        self.0.lock().unwrap().refusal.clone().map_or(Ok(()), Err)
    }
    fn read(&mut self, _: &Guest, f: ResourceFile, at: u64, len: usize) -> std::result::Result<Vec<u8>, String> {
        Ok(self.0.lock().unwrap().view(f, at, len))
    }
    fn held(&mut self, _: &Guest, f: ResourceFile, at: u64, len: u64) -> Vec<(u64, Vec<u8>)> {
        let s = self.0.lock().unwrap();
        (at..at + len)
            .filter_map(|p| s.overlay.get(&(key(f), p)).map(|b| (p, vec![*b])))
            .collect()
    }
    fn put(&mut self, _: &Guest, f: ResourceFile, at: u64, bytes: &[u8]) {
        let mut s = self.0.lock().unwrap();
        for (i, b) in bytes.iter().enumerate() {
            s.overlay.insert((key(f), at + i as u64), *b);
        }
    }
    fn restore(&mut self, _: &Guest, f: ResourceFile, at: u64, len: u64, runs: &[(u64, Vec<u8>)]) {
        let mut s = self.0.lock().unwrap();
        for p in at..at + len {
            s.overlay.remove(&(key(f), p));
        }
        for (p, bytes) in runs {
            for (i, b) in bytes.iter().enumerate() {
                s.overlay.insert((key(f), p + i as u64), *b);
            }
        }
    }
    fn append(&mut self, _: &Guest, f: ResourceFile, bytes: &[u8], align: u64) -> std::result::Result<u64, String> {
        let mut s = self.0.lock().unwrap();
        let own = s.own.get(&key(f)).map_or(0, |b| b.len() as u64);
        let end = s.appended.iter().filter(|a| a.0 == key(f)).map(|a| a.1 + a.2).max().unwrap_or(0);
        let at = end.max(own).div_ceil(align) * align;
        s.appended.push((key(f), at, bytes.len() as u64));
        for (i, b) in bytes.iter().enumerate() {
            s.overlay.insert((key(f), at + i as u64), *b);
        }
        Ok(at)
    }
    fn drop_appended(&mut self, _: &Guest, f: ResourceFile, at: u64) -> bool {
        let mut s = self.0.lock().unwrap();
        let Some(i) = s.appended.iter().position(|a| a.0 == key(f) && a.1 == at) else {
            return false;
        };
        let (_, at, len) = s.appended.remove(i);
        for p in at..at + len {
            s.overlay.remove(&(key(f), p));
        }
        true
    }
    fn clear(&mut self) {
        let mut s = self.0.lock().unwrap();
        s.overlay.clear();
        s.appended.clear();
    }
    fn summary(&self) -> Value {
        let s = self.0.lock().unwrap();
        json!({"entries": s.overlay.len(), "bytes": s.overlay.len(), "appended": s.appended.len()})
    }
}

fn entry(g: &mut Guest, slot: u32, group: &[u8; 4], datum: u32, addr: u32, size: u32) {
    let e = ENTRIES + slot * 16;
    g.set_u32(e, FourCC(*group).as_u32()).unwrap();
    g.set_u32(e + 4, datum).unwrap();
    g.set_u32(e + 8, addr).unwrap();
    g.set_u32(e + 12, size).unwrap();
}

/// The file's byte at `p`: its low byte, so every offset is told apart.
fn byte(p: u32) -> u8 {
    (p as u8) ^ 0x5a
}

/// Cluster `i`'s geometry block info address.
fn info(i: u32) -> u32 {
    CLUSTERS_AT + i * 0xb0 + 0x28
}

fn level() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW).unwrap();
    g.map_zeroed(META, 0x0001_0000, Permissions::RW).unwrap();
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(guest_layout::points::TAG_ENTRIES.base, ENTRIES).unwrap();
    g.set_u32(HEADER + index::HEADER_SCENARIO, SCNR).unwrap();
    g.set_u32(HEADER + index::HEADER_COUNT, 3).unwrap();
    entry(&mut g, 0, b"scnr", SCNR, META, 0x100);
    entry(&mut g, 1, b"sbsp", SBSP, SBSP_AT, 0x1200);
    entry(&mut g, 2, b"bitm", BITM, BITM_AT, 0x100);
    // Two clusters.
    g.set_u32(SBSP_AT + 0x9c, 2).unwrap();
    g.set_u32(SBSP_AT + 0xa0, CLUSTERS_AT).unwrap();
    for (i, block) in [(0, BLOCK0), (1, BLOCK1)] {
        g.set_u32(info(i), block).unwrap();
        g.set_u32(info(i) + 4, BLOCK_SIZE).unwrap();
        g.set_u16(info(i) + 0x1c, 0x24).unwrap();
        g.set_u32(info(i) + 0x20, u32::MAX).unwrap();
    }
    // Cluster 0 is paged in: its runtime section block names the section
    // struct after the 8-byte header of a buffer holding the file block.
    g.set_u8(info(0) + 0x1e, 1).unwrap();
    g.set_u32(info(0) + 0x20, 7).unwrap();
    g.set_u32(info(0) + 0x24, 1).unwrap();
    g.set_u32(info(0) + 0x28, RESIDENT + 8).unwrap();
    let block: Vec<u8> = (BLOCK0..BLOCK0 + BLOCK_SIZE).map(byte).collect();
    g.write_bytes(RESIDENT, &block, 1).unwrap();
    // One image whose LOD 0 lives in shared.map.
    g.set_u32(BITM_AT + 0x44, 1).unwrap();
    g.set_u32(BITM_AT + 0x48, IMAGES_AT).unwrap();
    g.set_u32(IMAGES_AT + 0x1c, 0x8000_0000 | PIXELS).unwrap();
    g.set_u32(IMAGES_AT + 0x34, 0x20).unwrap();
    g
}

fn files() -> Fake {
    let fake = Fake::default();
    {
        let mut s = fake.0.lock().unwrap();
        s.own.insert(0, (0..0x400).map(byte).collect());
        s.own.insert(2, (0..0x100).map(byte).collect());
    }
    fake
}

fn editor(fake: &Fake) -> (Editor, Arc<Mutex<Vec<Refresh>>>) {
    let pushed = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&pushed);
    let mut ed = Editor::new().with_raw(
        Box::new(fake.clone()),
        Box::new(move |r| sink.lock().unwrap().push(r)),
    );
    ed.at_epoch(&level(), 1, 1).unwrap();
    (ed, pushed)
}

fn cluster(i: u32) -> Value {
    json!({"section": {"tag": format!("{SBSP:#010x}"), "kind": "cluster", "index": i}})
}

fn write(owner: Value, offset: u32, bytes: &[u8], was: Option<Was>) -> RawBatch {
    RawBatch {
        raw: vec![RawWrite { owner, offset, bytes: bytes.to_vec(), was }],
        ..RawBatch::default()
    }
}

fn snapshot(g: &Guest) -> Vec<u8> {
    let mut b = vec![0; 0x1_0000];
    g.read_bytes(META, &mut b, 1).unwrap();
    b
}

#[test]
fn a_raw_write_changes_the_file_view_and_the_resident_copy_and_nothing_else() {
    let mut g = level();
    let fake = files();
    let (mut ed, pushed) = editor(&fake);
    let before = snapshot(&g);
    let old: Vec<u8> = (BLOCK0 + 0x10..BLOCK0 + 0x14).map(byte).collect();
    let reply = ed
        .batch_with(&mut g, &[], &write(cluster(0), 0x10, &[1, 2, 3, 4], Some(Was::Bytes(old))), None)
        .unwrap();
    assert_eq!(reply["raw"][0]["resident"], true, "{reply}");
    let s = fake.0.lock().unwrap();
    assert_eq!(s.view(ResourceFile::Local, u64::from(BLOCK0) + 0x10, 4), [1, 2, 3, 4]);
    assert_eq!(s.overlay.len(), 4, "only the written bytes are in the overlay");
    drop(s);
    let after = snapshot(&g);
    let at = (RESIDENT + 0x10 - META) as usize;
    assert_eq!(&after[at..at + 4], [1, 2, 3, 4]);
    let differs: Vec<usize> = (0..after.len()).filter(|&i| after[i] != before[i]).collect();
    assert_eq!(differs, (at..at + 4).collect::<Vec<_>>(), "no other guest byte moved");
    assert_eq!(
        *pushed.lock().unwrap(),
        [Refresh::Section { bsp: SBSP, kind: 0, index: 0, grown: false }]
    );
}

#[test]
fn a_was_mismatch_writes_nothing_at_all() {
    let mut g = level();
    let fake = files();
    let (mut ed, pushed) = editor(&fake);
    let before = snapshot(&g);
    // A field write in the same batch must not land either.
    let writes = [json!({"at": META + 0x10, "value": "aa"})];
    let e = ed
        .batch_with(&mut g, &writes, &write(cluster(0), 0, &[9; 4], Some(Was::Hash(1))), None)
        .unwrap_err();
    let BatchError::Mismatch { fields, raw, .. } = e else { panic!("not a mismatch") };
    assert!(fields.is_empty());
    assert_eq!(raw[0]["offset"], 0);
    assert_eq!(raw[0]["owner"], ed_owner_json(0));
    assert_eq!(snapshot(&g), before);
    assert!(fake.0.lock().unwrap().overlay.is_empty());
    assert!(pushed.lock().unwrap().is_empty());
    // A resident copy that differs from the file view is a mismatch too.
    g.write_bytes(RESIDENT, &[0xee], 1).unwrap();
    let e = ed.batch_with(&mut g, &[], &write(cluster(0), 0, &[9], None), None).unwrap_err();
    let BatchError::Mismatch { raw, .. } = e else { panic!("not a mismatch") };
    assert_eq!(raw[0]["why"], "the resident copy differs");
    assert!(fake.0.lock().unwrap().overlay.is_empty());
}

fn ed_owner_json(i: u32) -> Value {
    Owner::Section { tag: SBSP, kind: SectionKind::Cluster, index: i }.json()
}

#[test]
fn revert_restores_the_file_view_and_the_resident_copy() {
    let mut g = level();
    let fake = files();
    let (mut ed, pushed) = editor(&fake);
    let before = snapshot(&g);
    // An earlier overlay entry under the range survives the revert.
    fake.0.lock().unwrap().overlay.insert((0, u64::from(BLOCK0)), 0x77);
    g.write_bytes(RESIDENT, &[0x77], 1).unwrap();
    let id = ed
        .batch_with(&mut g, &[], &write(cluster(0), 0, &[1, 2], None), None)
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let changes = ed.changes(&g);
    assert_eq!(changes["raw"][0]["id"], id, "{changes}");
    let r = ed.revert(&mut g, Some(id)).unwrap();
    assert_eq!(r["reverted"][0]["resident"], true, "{r}");
    let mut want = before.clone();
    want[(RESIDENT - META) as usize] = 0x77;
    assert_eq!(snapshot(&g), want);
    let s = fake.0.lock().unwrap();
    assert_eq!(s.overlay.len(), 1);
    assert_eq!(s.view(ResourceFile::Local, u64::from(BLOCK0), 2), [0x77, byte(BLOCK0 + 1)]);
    drop(s);
    assert_eq!(pushed.lock().unwrap().len(), 2, "the write's refresh and the revert's");
    assert!(ed.changes(&g)["raw"].as_array().unwrap().is_empty());
}

#[test]
fn a_non_resident_owner_takes_the_overlay_only() {
    let mut g = level();
    let fake = files();
    let (mut ed, _) = editor(&fake);
    let before = snapshot(&g);
    let reply = ed.batch_with(&mut g, &[], &write(cluster(1), 4, &[5, 6], None), None).unwrap();
    assert_eq!(reply["raw"][0]["resident"], false, "{reply}");
    assert_eq!(snapshot(&g), before);
    assert_eq!(fake.0.lock().unwrap().view(ResourceFile::Local, u64::from(BLOCK1) + 4, 2), [5, 6]);
    // Pixels have no resident copy yet: the overlay of shared.map only.
    let pixels = json!({"pixels": {"tag": format!("{BITM:#010x}"), "image": 0, "lod": 0}});
    let reply = ed.batch_with(&mut g, &[], &write(pixels, 0, &[8], None), None).unwrap();
    assert_eq!(reply["raw"][0]["refresh"]["image"]["image"], 0, "{reply}");
    assert_eq!(fake.0.lock().unwrap().view(ResourceFile::Shared, u64::from(PIXELS), 1), [8]);
    assert_eq!(snapshot(&g), before);
}

#[test]
fn writes_outside_the_owners_block_and_unready_readers_are_refused() {
    let mut g = level();
    let fake = files();
    let (mut ed, _) = editor(&fake);
    let e = ed.batch_with(&mut g, &[], &write(cluster(0), BLOCK_SIZE - 1, &[1, 2], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    let e = ed.batch_with(&mut g, &[], &write(cluster(2), 0, &[1], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(_)), "{e:?}");
    fake.0.lock().unwrap().refusal = Some("no slot".into());
    let e = ed.batch_with(&mut g, &[], &write(cluster(0), 0, &[1], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Unavailable, .. })), "{e:?}");
    assert!(fake.0.lock().unwrap().overlay.is_empty());
    // An editor without raw files says so.
    let e = Editor::new().batch_with(&mut g, &[], &write(cluster(0), 0, &[1], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Unavailable, .. })), "{e:?}");
}

#[test]
fn an_eviction_marks_the_section_unloaded_and_revert_evicts_it_again() {
    let mut g = level();
    let fake = files();
    let (mut ed, pushed) = editor(&fake);
    let batch = RawBatch { evict: vec![cluster(0)], ..RawBatch::default() };
    // The block info's pointer moves to an appended block in the same batch.
    let appended = ed.raw_append(&g, ResourceFile::Local, &[0xab; 8], 16).unwrap();
    let pointer = appended["pointer"].as_u64().unwrap() as u32;
    assert_eq!(pointer, 0x400, "past the file's end, aligned");
    let writes = [json!({"at": info(0), "value": hex(&pointer.to_le_bytes())})];
    let reply = ed.batch_with(&mut g, &writes, &batch, None).unwrap();
    assert_eq!(reply["evicted"][0], ed_owner_json(0), "{reply}");
    assert_eq!(g.u8(info(0) + 0x1e).unwrap(), 0);
    assert_eq!(g.u32(info(0) + 0x20).unwrap(), u32::MAX);
    assert_eq!(g.u32(info(0) + 0x24).unwrap(), 0);
    assert_eq!(g.u32(info(0)).unwrap(), 0x400);
    assert_eq!(
        *pushed.lock().unwrap(),
        [Refresh::Section { bsp: SBSP, kind: 0, index: 0, grown: true }]
    );
    // The guest pages it in again.
    g.set_u8(info(0) + 0x1e, 1).unwrap();
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(g.u32(info(0)).unwrap(), BLOCK0, "{r}");
    assert_eq!(g.u8(info(0) + 0x1e).unwrap(), 0, "evicted again to page the old block in");
    let s = fake.0.lock().unwrap();
    assert!(s.appended.is_empty() && s.overlay.is_empty());
}

#[test]
fn a_new_map_drops_the_overlay_and_the_raw_journal() {
    let mut g = level();
    let fake = files();
    let (mut ed, _) = editor(&fake);
    ed.batch_with(&mut g, &[], &write(cluster(1), 0, &[1], None), None).unwrap();
    assert_eq!(ed.overlay()["owners"].as_array().unwrap().len(), 1);
    ed.at_epoch(&g, 5, 5).unwrap();
    assert!(fake.0.lock().unwrap().overlay.is_empty());
    assert!(ed.overlay()["owners"].as_array().unwrap().is_empty());
}

/// The scenario's structure BSP block: two BSPs whose regions share one
/// virtual base, at their own file offsets, BSP 0 loaded.
const BSPS_AT: u32 = META + 0x5000;
const REGION: [u32; 2] = [0x300, 0x340];
const REGION_SIZE: u32 = 0x40;
const REGION_BASE: u32 = 0x2000_0000;

fn two_bsps() -> Guest {
    let mut g = level();
    g.set_u32(META + 0x210, 2).unwrap();
    g.set_u32(META + 0x214, BSPS_AT).unwrap();
    for (i, file) in REGION.into_iter().enumerate() {
        let d = BSPS_AT + i as u32 * 0x44;
        g.set_u32(d, file).unwrap();
        g.set_u32(d + 4, REGION_SIZE).unwrap();
        g.set_u32(d + 8, REGION_BASE).unwrap();
    }
    g.map_zeroed(REGION_BASE, 0x1000, Permissions::RW).unwrap();
    switch_to(&mut g, &files(), 0);
    g
}

/// What a switch does to memory: the loaded index, and the region read
/// from the file view at its virtual base.
fn switch_to(g: &mut Guest, fake: &Fake, i: usize) {
    g.set_u16(engine::sim_collision::layout::BSP_INDEX, i as u16).unwrap();
    let bytes = fake.0.lock().unwrap().view(ResourceFile::Local, u64::from(REGION[i]), REGION_SIZE as usize);
    g.write_bytes(REGION_BASE, &bytes, 1).unwrap();
}

fn region(g: &Guest) -> Vec<u8> {
    let mut b = vec![0; REGION_SIZE as usize];
    g.read_bytes(REGION_BASE, &mut b, 1).unwrap();
    b
}

fn bsp(i: u32) -> Value {
    json!({"bsp": {"index": i}})
}

#[test]
fn a_write_to_an_inactive_bsp_is_held_by_the_overlay_and_not_written_to_guest_memory() {
    let mut g = two_bsps();
    let fake = files();
    let (mut ed, pushed) = editor(&fake);
    let (before, loaded) = (snapshot(&g), region(&g));
    let old: Vec<u8> = (REGION[1] + 0x10..REGION[1] + 0x14).map(byte).collect();
    let reply = ed
        .batch_with(&mut g, &[], &write(bsp(1), 0x10, &[1, 2, 3, 4], Some(Was::Bytes(old))), None)
        .unwrap();
    assert_eq!(reply["raw"][0]["owner"], bsp(1), "{reply}");
    assert_eq!(reply["raw"][0]["resident"], false);
    assert!(reply["raw"][0]["refresh"].is_null(), "the switch loads the world from the file");
    let s = fake.0.lock().unwrap();
    assert_eq!(s.view(ResourceFile::Local, u64::from(REGION[1]) + 0x10, 4), [1, 2, 3, 4]);
    assert_eq!(s.overlay.len(), 4);
    drop(s);
    assert_eq!(snapshot(&g), before, "no tag byte moved");
    assert_eq!(region(&g), loaded, "the loaded BSP's region is untouched");
    assert!(pushed.lock().unwrap().is_empty());
    assert_eq!(ed.overlay()["owners"], json!([bsp(1)]));
    // The next switch reads the edit.
    switch_to(&mut g, &fake, 1);
    assert_eq!(&region(&g)[0x10..0x14], [1, 2, 3, 4]);
}

#[test]
fn the_loaded_bsp_and_a_bsp_past_the_block_are_refused() {
    let mut g = two_bsps();
    let fake = files();
    let (mut ed, _) = editor(&fake);
    for owner in [bsp(0), bsp(2)] {
        let e = ed.batch_with(&mut g, &[], &write(owner, 0, &[1], None), None).unwrap_err();
        assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    }
    let e = ed.batch_with(&mut g, &[], &write(bsp(1), REGION_SIZE - 1, &[1, 2], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    let e = ed
        .batch_with(&mut g, &[], &write(bsp(1), 0, &[1], Some(Was::Bytes(vec![0xee]))), None)
        .unwrap_err();
    let BatchError::Mismatch { raw, .. } = e else { panic!("not a mismatch") };
    assert_eq!(raw[0]["owner"], bsp(1));
    assert!(fake.0.lock().unwrap().overlay.is_empty());
}

#[test]
fn revert_of_an_inactive_bsp_write_restores_the_file_and_the_bsp_once_switched_to() {
    let mut g = two_bsps();
    let fake = files();
    let (mut ed, _) = editor(&fake);
    let file: Vec<u8> = (REGION[1]..REGION[1] + REGION_SIZE).map(byte).collect();
    // Not switched to: the overlay alone goes back.
    let id = ed.batch_with(&mut g, &[], &write(bsp(1), 4, &[9, 9], None), None).unwrap()["id"].as_u64().unwrap();
    let r = ed.revert(&mut g, Some(id)).unwrap();
    assert_eq!(r["reverted"][0]["resident"], false, "{r}");
    assert!(fake.0.lock().unwrap().overlay.is_empty());
    // Switched to since: the loaded region holds the edit until the revert.
    ed.batch_with(&mut g, &[], &write(bsp(1), 4, &[9, 9], None), None).unwrap();
    switch_to(&mut g, &fake, 1);
    assert_eq!(&region(&g)[4..6], [9, 9]);
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["reverted"][0]["resident"], true, "{r}");
    assert_eq!(region(&g), file);
    assert!(fake.0.lock().unwrap().overlay.is_empty());
}

#[test]
fn fnv1a64_matches_the_reference_values() {
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
}

const SND: u32 = 0xe333_0003;
const UGH: u32 = 0xe444_0004;
const SND_AT: u32 = META + 0x5000;
const UGH_AT: u32 = META + 0x6000;
const RANGES_AT: u32 = META + 0x6100;
const PERMS_AT: u32 = META + 0x6200;
const CHUNKS_AT: u32 = META + 0x6300;
/// The sound's two chunks: one loaded (local, handle 5), one not (shared).
const CHUNK0: u32 = 0x300;
const CHUNK1: u32 = 0x80;
const CHUNK_LEN: u32 = 0x20;

/// Chunk `i` of the gestalt's block: the sound's chunk 0 is element 1.
fn chunk_record(i: u32) -> u32 {
    CHUNKS_AT + i * 12
}

/// [`level`] with a `snd!` and the map's `ugh!`. The sound's one pitch
/// range is the gestalt's range 1, its one permutation the gestalt's
/// permutation 1, whose chunks are the gestalt's chunks 1 and 2; element 0
/// of each block belongs to another sound.
fn sound_level() -> Guest {
    let mut g = level();
    g.set_u32(HEADER + index::HEADER_COUNT, 5).unwrap();
    entry(&mut g, 3, b"snd!", SND, SND_AT, 0x14);
    entry(&mut g, 4, b"ugh!", UGH, UGH_AT, 0x58);
    g.set_u16(SND_AT + 0x8, 1).unwrap();
    g.set_u8(SND_AT + 0xa, 1).unwrap();
    for (block, count, at) in [(4, 2, RANGES_AT), (5, 2, PERMS_AT), (8, 3, CHUNKS_AT)] {
        g.set_u32(UGH_AT + block * 8, count).unwrap();
        g.set_u32(UGH_AT + block * 8 + 4, at).unwrap();
    }
    g.set_u16(RANGES_AT + 12 + 8, 1).unwrap();
    g.set_u16(RANGES_AT + 12 + 10, 1).unwrap();
    g.set_u16(PERMS_AT + 16 + 12, 1).unwrap();
    g.set_u16(PERMS_AT + 16 + 14, 2).unwrap();
    // Chunk 1: local, bit 31 of its size set, loaded under handle 5.
    g.set_u32(chunk_record(1), CHUNK0).unwrap();
    g.set_u32(chunk_record(1) + 4, 0x8000_0000 | CHUNK_LEN).unwrap();
    g.set_u32(chunk_record(1) + 8, 5).unwrap();
    // Chunk 2: shared, not loaded.
    g.set_u32(chunk_record(2), 0x8000_0000 | CHUNK1).unwrap();
    g.set_u32(chunk_record(2) + 4, CHUNK_LEN).unwrap();
    g.set_u32(chunk_record(2) + 8, u32::MAX).unwrap();
    g
}

fn sound_editor(fake: &Fake) -> (Editor, Arc<Mutex<Vec<Refresh>>>) {
    let pushed = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&pushed);
    let mut ed = Editor::new().with_raw(
        Box::new(fake.clone()),
        Box::new(move |r| sink.lock().unwrap().push(r)),
    );
    ed.at_epoch(&sound_level(), 1, 1).unwrap();
    (ed, pushed)
}

fn samples(range: u32, permutation: u32, chunk: u32) -> Value {
    json!({"sound": {"tag": format!("{SND:#010x}"), "range": range,
        "permutation": permutation, "chunk": chunk}})
}

#[test]
fn a_sound_chunk_write_takes_the_overlay_alone() {
    let mut g = sound_level();
    let fake = files();
    let (mut ed, pushed) = sound_editor(&fake);
    let before = snapshot(&g);
    let old: Vec<u8> = (CHUNK0 + 4..CHUNK0 + 8).map(byte).collect();
    let reply = ed
        .batch_with(&mut g, &[], &write(samples(0, 0, 0), 4, &[1, 2, 3, 4], Some(Was::Bytes(old))), None)
        .unwrap();
    assert_eq!(reply["raw"][0]["resident"], false, "{reply}");
    assert_eq!(reply["raw"][0]["refresh"], Value::Null, "{reply}");
    assert_eq!(reply["raw"][0]["owner"], samples(0, 0, 0), "{reply}");
    assert_eq!(fake.0.lock().unwrap().view(ResourceFile::Local, u64::from(CHUNK0) + 4, 4), [1, 2, 3, 4]);
    // The second chunk lives in shared.map.
    ed.batch_with(&mut g, &[], &write(samples(0, 0, 1), 0, &[9], None), None).unwrap();
    assert_eq!(fake.0.lock().unwrap().view(ResourceFile::Shared, u64::from(CHUNK1), 1), [9]);
    assert_eq!(snapshot(&g), before, "no guest byte moved: the samples have no resident copy");
    assert!(pushed.lock().unwrap().is_empty(), "sounds queue no native refresh");
    assert_eq!(ed.overlay()["owners"].as_array().unwrap().len(), 2);
}

#[test]
fn a_sound_chunk_is_bounded_by_its_masked_size_and_its_runs() {
    let mut g = sound_level();
    let fake = files();
    let (mut ed, _) = sound_editor(&fake);
    // Bit 31 of the size is a flag: the block is CHUNK_LEN bytes.
    let e = ed.batch_with(&mut g, &[], &write(samples(0, 0, 0), CHUNK_LEN - 1, &[1, 2], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    // Past the sound's one range, its permutation's two chunks.
    for owner in [samples(1, 0, 0), samples(0, 1, 0), samples(0, 0, 2)] {
        let e = ed.batch_with(&mut g, &[], &write(owner, 0, &[1], None), None).unwrap_err();
        assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
    }
    // A bitmap is not a sound.
    let wrong = json!({"sound": {"tag": format!("{BITM:#010x}"), "range": 0, "permutation": 0, "chunk": 0}});
    let e = ed.batch_with(&mut g, &[], &write(wrong, 0, &[1], None), None).unwrap_err();
    assert!(matches!(e, BatchError::Field(_)), "{e:?}");
    assert!(fake.0.lock().unwrap().overlay.is_empty());
}

#[test]
fn a_map_without_a_gestalt_refuses_sound_owners() {
    let mut g = sound_level();
    g.set_u32(HEADER + index::HEADER_COUNT, 4).unwrap();
    let fake = files();
    let (mut ed, _) = sound_editor(&fake);
    let e = ed.batch_with(&mut g, &[], &write(samples(0, 0, 0), 0, &[1], None), None).unwrap_err();
    let BatchError::Field(e) = e else { panic!("not a field error: {e:?}") };
    assert!(e.message.contains("ugh!"), "{e:?}");
}

#[test]
fn a_sound_eviction_clears_the_cache_handle_and_revert_clears_it_again() {
    let mut g = sound_level();
    let fake = files();
    let (mut ed, pushed) = sound_editor(&fake);
    let batch = RawBatch {
        raw: vec![RawWrite { owner: samples(0, 0, 0), offset: 0, bytes: vec![7; 4], was: None }],
        evict: vec![samples(0, 0, 0)],
        ..RawBatch::default()
    };
    let reply = ed.batch_with(&mut g, &[], &batch, None).unwrap();
    assert_eq!(reply["evicted"][0]["sound"]["chunk"], 0, "{reply}");
    assert_eq!(g.u32(chunk_record(1) + 8).unwrap(), u32::MAX);
    assert_eq!(g.u32(chunk_record(1)).unwrap(), CHUNK0, "the pointer stays");
    assert_eq!(g.u32(chunk_record(2) + 8).unwrap(), u32::MAX);
    assert!(pushed.lock().unwrap().is_empty());
    // The guest plays the sound again and loads the chunk under a new handle.
    g.set_u32(chunk_record(1) + 8, 9).unwrap();
    let r = ed.revert(&mut g, None).unwrap();
    assert_eq!(r["reverted"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(g.u32(chunk_record(1) + 8).unwrap(), u32::MAX, "evicted again to load the old samples");
    assert!(fake.0.lock().unwrap().overlay.is_empty());
}

#[test]
fn pixels_cannot_be_evicted() {
    let mut g = sound_level();
    let fake = files();
    let (mut ed, _) = sound_editor(&fake);
    let pixels = json!({"pixels": {"tag": format!("{BITM:#010x}"), "image": 0, "lod": 0}});
    let batch = RawBatch { evict: vec![pixels], ..RawBatch::default() };
    let e = ed.batch_with(&mut g, &[], &batch, None).unwrap_err();
    assert!(matches!(e, BatchError::Field(Error { kind: ErrorKind::Invalid, .. })), "{e:?}");
}

fn hex(b: &[u8]) -> String {
    value::hex(b)
}
