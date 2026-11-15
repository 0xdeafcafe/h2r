//! Synthetic maps only: every byte, name and layout below is invented.
use super::*;
use audio::VoiceOutput;
use dsound::padroute::Classified;
use std::path::PathBuf;
use std::time::Duration;

const INDEX: usize = 0x1000;
const INDEX_VA: u32 = 0x8000_0000;
const META_VA: u32 = 0x9000_0000;
/// Where the raw sample bytes start in a synthetic map file.
const RAW: usize = 0x8000;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// One sound: a name and its permutations, each a list of
/// `(raw pointer, size)` chunks.
type Sound<'a> = (&'a str, Vec<Vec<(u32, u32)>>);

/// A map-like file the `map` crate parses: a tag index of one `ugh!` gestalt
/// and a `snd!` tag per sound, then `raw` at [`RAW`] (local pointers are
/// file offsets, so a local chunk at `RAW + n` points `n` bytes into `raw`).
fn build_map(sounds: &[Sound<'_>], raw: &[u8]) -> Vec<u8> {
    // The gestalt: one pitch range per sound.
    let (mut ranges, mut perms, mut chunks) = (Vec::new(), Vec::new(), Vec::new());
    let mut snd = Vec::new();
    for (i, (_, ps)) in sounds.iter().enumerate() {
        let mut h = [0u8; 20];
        h[8..10].copy_from_slice(&(i as u16).to_le_bytes());
        h[10] = 1;
        snd.push(h);
        ranges.extend([0u8; 8]);
        ranges.extend(((perms.len() / 16) as u16).to_le_bytes());
        ranges.extend((ps.len() as u16).to_le_bytes());
        for p in ps {
            perms.extend([0u8; 8]);
            perms.extend(p.iter().map(|c| c.1).sum::<u32>().to_le_bytes());
            perms.extend(((chunks.len() / 12) as u16).to_le_bytes());
            perms.extend((p.len() as u16).to_le_bytes());
            for &(ptr, size) in p {
                chunks.extend(ptr.to_le_bytes());
                chunks.extend(size.to_le_bytes());
                chunks.extend(u32::MAX.to_le_bytes());
            }
        }
    }
    let mut ugh = vec![0u8; 0x58];
    for (idx, count, body) in [
        (4usize, ranges.len() / 12, &ranges),
        (5, perms.len() / 16, &perms),
        (8, chunks.len() / 12, &chunks),
    ] {
        let addr = META_VA + ugh.len() as u32;
        put(&mut ugh, idx * 8, count as u32);
        put(&mut ugh, idx * 8 + 4, addr);
        ugh.extend_from_slice(body);
    }
    // Tags: the gestalt, then the sounds.
    let mut payloads: Vec<(FourCC, String, Vec<u8>)> =
        vec![(FourCC(*b"ugh!"), "sound\\gestalt".into(), ugh)];
    for ((name, _), h) in sounds.iter().zip(&snd) {
        payloads.push((FourCC(*b"snd!"), name.to_string(), h.to_vec()));
    }
    let groups = [FourCC(*b"ugh!"), FourCC(*b"snd!")];
    let index_size = 32 + groups.len() * 12 + payloads.len() * 16;
    let meta = INDEX + index_size;
    let mut b = vec![0u8; RAW];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    put(&mut b, 4, 8);
    put(&mut b, 0x10, INDEX as u32);
    put(&mut b, 0x14, index_size as u32);
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    b[0x198..0x19d].copy_from_slice(b"synth");
    // Tag names.
    let mut blob = Vec::new();
    for (i, (_, name, _)) in payloads.iter().enumerate() {
        put(&mut b, 0x900 + 4 * i, blob.len() as u32);
        blob.extend(name.as_bytes());
        blob.push(0);
    }
    b[0x800..0x800 + blob.len()].copy_from_slice(&blob);
    put(&mut b, 0x2c0, payloads.len() as u32);
    put(&mut b, 0x2c4, 0x800);
    put(&mut b, 0x2c8, blob.len() as u32);
    put(&mut b, 0x2cc, 0x900);
    // Index header, groups, tags.
    put(&mut b, INDEX, INDEX_VA + 32);
    put(&mut b, INDEX + 4, groups.len() as u32);
    put(&mut b, INDEX + 8, INDEX_VA + 32 + groups.len() as u32 * 12);
    put(&mut b, INDEX + 12, u32::MAX);
    put(&mut b, INDEX + 16, u32::MAX);
    put(&mut b, INDEX + 24, payloads.len() as u32);
    put(&mut b, INDEX + 28, FourCC(*b"tags").as_u32());
    for (i, g) in groups.iter().enumerate() {
        let at = INDEX + 32 + i * 12;
        put(&mut b, at, g.as_u32());
        put(&mut b, at + 4, u32::MAX);
        put(&mut b, at + 8, u32::MAX);
    }
    let mut at = meta;
    for (i, (group, _, body)) in payloads.iter().enumerate() {
        let t = INDEX + 32 + groups.len() * 12 + i * 16;
        put(&mut b, t, group.as_u32());
        put(&mut b, t + 4, 0xe000_0000 | i as u32);
        put(&mut b, t + 8, META_VA + (at - meta) as u32);
        put(&mut b, t + 12, body.len() as u32);
        b[at..at + body.len()].copy_from_slice(body);
        at += body.len();
    }
    assert!(at <= RAW, "metadata fits before the raw data");
    put(&mut b, 0x18, (at - meta) as u32);
    b.extend_from_slice(raw);
    let len = b.len() as u32;
    put(&mut b, 8, len);
    b
}

/// Synthetic encoded bytes from a seed.
fn bytes(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| {
            (i as u8)
                .wrapping_mul(29)
                .wrapping_add(seed.wrapping_mul(101))
                ^ (i >> 8) as u8
        })
        .collect()
}

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let p = base.join(format!("h2-pad-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn whole(path: &std::path::Path) -> HostFile {
    HostFile {
        path: path.to_path_buf(),
        offset: 0,
        len: std::fs::metadata(path).unwrap().len(),
    }
}

/// A shared map with one sound of its own and raw data a level points into,
/// and a level whose sounds use local chunks (one split across two chunks)
/// and a chunk in the shared map.
struct Maps {
    shared: Vec<u8>,
    level: Vec<u8>,
    reload: Vec<u8>,
    shield: Vec<u8>,
    music: Vec<u8>,
    hit: Vec<u8>,
}

fn maps() -> Maps {
    let (reload, music) = (bytes(1, 900), bytes(2, 700));
    // shared.map: reload at raw+0, music (used by the level) at raw+900.
    let shared_raw = [reload.clone(), music.clone()].concat();
    let shared = build_map(
        &[("sfx\\weapons\\reload", vec![vec![(RAW as u32, 900)]])],
        &shared_raw,
    );
    // The level: a shield sound in two local chunks (400 + 400 bytes, so its
    // 576-byte prefix spans both), a short 100-byte hit, and the music.
    let (shield, hit) = (bytes(3, 800), bytes(4, 100));
    let level_raw = [shield.clone(), hit.clone()].concat();
    let shared_ptr = 0x8000_0000 | (RAW as u32 + 900);
    let level = build_map(
        &[
            (
                "sfx\\player\\shield_down",
                vec![vec![(RAW as u32, 400), (RAW as u32 + 400, 400)]],
            ),
            ("sfx\\player\\hit", vec![vec![(RAW as u32 + 800, 100)]]),
            ("music\\level\\theme", vec![vec![(shared_ptr, 700)]]),
        ],
        &level_raw,
    );
    Maps {
        shared,
        level,
        reload,
        shield,
        music,
        hit,
    }
}

fn config() -> PadSpeakerConfig {
    PadSpeakerConfig {
        enabled: true,
        routes: vec!["*shield*".into(), "*reload*".into(), "*hit".into()],
        ..PadSpeakerConfig::default()
    }
}

#[test]
fn a_synthetic_map_parses_and_indexes_every_permutation() {
    let m = maps();
    let mut map = Map::from_reader(std::io::Cursor::new(m.level.clone())).unwrap();
    assert_eq!(map.tags().len(), 4);
    struct Mem(Vec<u8>, Vec<u8>);
    impl ResourceReader for Mem {
        fn check(&self, _: RawPointer, _: u32) -> tags::Result<()> {
            Ok(())
        }
        fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
            let f = if p.file == ResourceFile::Shared {
                &self.1
            } else {
                &self.0
            };
            Ok(f[p.offset as usize..][..size as usize].to_vec())
        }
    }
    let mut index = SoundIndex::new();
    let n = index_map(
        &mut index,
        &mut map,
        &mut Mem(m.level.clone(), m.shared.clone()),
    )
    .unwrap();
    assert_eq!(
        n,
        Indexed {
            tags: 3,
            permutations: 3,
            skipped: 0
        }
    );
    assert_eq!(index.lookup(&m.shield), ["sfx\\player\\shield_down"]);
    assert_eq!(index.lookup(&m.hit), ["sfx\\player\\hit"]);
    assert_eq!(index.lookup(&m.music), ["music\\level\\theme"]);
    // The middle of a sound is not its start.
    assert!(index.lookup(&m.shield[400..]).is_empty());
}

#[test]
fn levels_and_shared_maps_are_indexed_from_their_files() {
    let t = Temp::new("files");
    let (level, shared) = (t.0.join("level.map"), t.0.join("shared.map"));
    let m = maps();
    std::fs::write(&level, &m.level).unwrap();
    std::fs::write(&shared, &m.shared).unwrap();
    let mut ix = PadIndexer::new(&config(), [None, Some(whole(&shared)), None]);
    let r = ix.add_shared().expect("the shared map has a sound");
    assert_eq!(r.classify(&m.reload), Classified::Start(VoiceOutput::Side));
    assert_eq!(
        r.classify(&m.shield),
        Classified::Unknown,
        "level not yet loaded"
    );
    let r = ix
        .add(&whole(&level))
        .unwrap()
        .expect("the level has sounds");
    assert_eq!(r.classify(&m.shield), Classified::Start(VoiceOutput::Side));
    assert_eq!(r.classify(&m.hit), Classified::Start(VoiceOutput::Side));
    // Read from shared.map through the level's shared pointer; not routed.
    assert_eq!(r.classify(&m.music), Classified::Start(VoiceOutput::Main));
    // The shared map's sounds stay indexed.
    assert_eq!(r.classify(&m.reload), Classified::Start(VoiceOutput::Side));
    assert_eq!(ix.tag_count(), 4);
    // A map seen before adds nothing.
    assert!(ix.add(&whole(&level)).unwrap().is_none());
}

#[test]
fn maps_inside_a_disc_image_are_read_from_their_region() {
    let t = Temp::new("image");
    let m = maps();
    // An "image": junk, the shared map, junk, the level.
    let mut image = bytes(9, 0x3000);
    let shared_at = image.len() as u64;
    image.extend(&m.shared);
    image.extend(bytes(8, 0x1234));
    let level_at = image.len() as u64;
    image.extend(&m.level);
    image.extend(bytes(7, 0x100));
    let path = t.0.join("disc.iso");
    std::fs::write(&path, &image).unwrap();
    let region = |offset, len: usize| HostFile {
        path: path.clone(),
        offset,
        len: len as u64,
    };
    let mut ix = PadIndexer::new(
        &config(),
        [None, Some(region(shared_at, m.shared.len())), None],
    );
    let r = ix.add(&region(level_at, m.level.len())).unwrap().unwrap();
    assert_eq!(r.classify(&m.shield), Classified::Start(VoiceOutput::Side));
    assert_eq!(r.classify(&m.music), Classified::Start(VoiceOutput::Main));
}

#[test]
fn a_missing_shared_map_skips_only_the_sounds_that_need_it() {
    let t = Temp::new("missing");
    let level = t.0.join("level.map");
    let m = maps();
    std::fs::write(&level, &m.level).unwrap();
    let mut ix = PadIndexer::new(&config(), [None, None, None]);
    let r = ix.add(&whole(&level)).unwrap().unwrap();
    assert_eq!(r.classify(&m.shield), Classified::Start(VoiceOutput::Side));
    assert_eq!(r.classify(&m.music), Classified::Unknown);
    // Not a map at all: an error, nothing installed.
    let junk = t.0.join("junk.map");
    std::fs::write(&junk, bytes(5, 4096)).unwrap();
    assert!(ix.add(&whole(&junk)).is_err());
}

#[test]
fn opening_a_map_through_the_vfs_installs_a_router() {
    let t = Temp::new("vfs");
    let m = maps();
    let cfg = vfs::Config::new(&t.0, 0x4d53_0064);
    let v = vfs::Vfs::with_storage(cfg).unwrap();
    let dir = t.0.join("disk").join("Partition1").join("maps");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("level.map"), &m.level).unwrap();
    std::fs::write(dir.join("shared.map"), &m.shared).unwrap();
    std::fs::write(dir.join("notes.txt"), b"not a map").unwrap();
    let v = Arc::new(Mutex::new(v));
    let (tx, rx) = mpsc::channel();
    let p1 = "\\Device\\Harddisk0\\Partition1\\maps";
    let index = spawn(
        Arc::clone(&v),
        &config(),
        [
            &format!("{p1}\\mainmenu.map"),
            &format!("{p1}\\shared.map"),
            "\\Device\\Missing\\x.map",
        ],
        move |r| {
            let _ = tx.send(r);
        },
    );
    // The shared map first, at start.
    let r = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(r.classify(&m.reload), Classified::Start(VoiceOutput::Side));
    // The game opens a text file (ignored) and the level (indexed).
    let open = |name: &str| {
        v.lock()
            .unwrap()
            .open(name, vfs::OpenOptions::default())
            .unwrap()
    };
    let _ = open(&format!("{p1}\\notes.txt"));
    let _ = open(&format!("{p1}\\LEVEL.MAP"));
    let r = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(r.classify(&m.shield), Classified::Start(VoiceOutput::Side));
    assert_eq!(r.classify(&m.music), Classified::Start(VoiceOutput::Main));
    // Opening it again, or the shared map, builds nothing new.
    let _ = open(&format!("{p1}\\level.map"));
    let _ = open(&format!("{p1}\\shared.map"));
    drop(index);
    assert!(rx.try_recv().is_err());
}
