//! The game's rewrite of a map larger than any buffer it may hold: an
//! invented 200 MiB campaign map (a raw resource of that size, two tags)
//! and a package that adds a tag. The streamed rewrite the VFS serves is
//! byte-equal to the buffered build (`workspace::build_map`), and the
//! rewrite's peak memory is bounded by what it changes, not by the map.
//! No disc bytes.
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use content::Sha256;
use content::package::{Lock, Manifest, MapScope, Package};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC};
use tags::defs::{Defs, default_record, to_built};
use vfs::{Backend, BasicInfo, DirectoryEntry, Error, Metadata, Result, RewriteBackend, VolumeInfo};
use workspace::{Loaded, PackageRewrite, Target, build_map};

/// Live heap bytes, and the most there have been since [`reset_peak`].
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(l.size(), Ordering::Relaxed) + l.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if n > l.size() {
            let live = LIVE.fetch_add(n - l.size(), Ordering::Relaxed) + n - l.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(l.size() - n, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn reset_peak() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    live
}

const MIB: usize = 1 << 20;
const PATH: &str = "maps\\big.map";
const C: &str = "invented\\dmg\\c";
const ADD_C_FILE: &str = "dmg_c.jpt!.yaml";

fn cc(s: &[u8; 4]) -> FourCC {
    FourCC(*s)
}

/// The invented map: globals, one damage tag, and `raw` bytes of raw data
/// that nothing changes.
fn big_map(raw: usize) -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Campaign, "big", "invented\\scenarios\\big");
    for g in [b"matg", b"jpt!"] {
        set.add_group(cc(g), None, None);
    }
    set.add_tag(cc(b"matg"), "globals\\globals", TagPayload::Built(Block::zeroed(0x40)))
        .unwrap();
    let jpt = Defs::builtin().group(cc(b"jpt!")).unwrap();
    let block = to_built(jpt, &default_record(jpt), &mut |_| None).unwrap();
    set.add_tag(cc(b"jpt!"), "invented\\dmg\\a", TagPayload::Built(block))
        .unwrap();
    let bytes = (0..raw)
        .map(|i| (i as u32).wrapping_mul(0x9e37_79b9).rotate_left(11) as u8)
        .collect();
    set.add_raw(bytes, 0x1000);
    set.build().unwrap().0
}

/// A package that adds a damage tag to every map.
fn adds_c() -> Loaded {
    let text = format!(
        "[package]\nformat = 1\nid = \"c\"\nname = \"c\"\n[[item]]\nkind = \"tag\"\nsource = \"{ADD_C_FILE}\"\ntag = '{C}'\nmaps = \"all\"\n"
    );
    let manifest = Manifest::from_toml(&text).unwrap();
    let files: BTreeMap<String, Vec<u8>> = [(
        ADD_C_FILE.to_owned(),
        b"fields:\n  damage_lower_bound: 5.0\n".to_vec(),
    )]
    .into();
    let lock = Lock::new(files.iter().map(|(n, b)| (n.clone(), Sha256::of(b))).collect());
    Loaded {
        package: Package::new(manifest, lock).unwrap(),
        files,
    }
}

/// One map file, served by offset.
struct Disc {
    map: Arc<Vec<u8>>,
}

fn meta(len: u64, dir: bool) -> Metadata {
    Metadata {
        basic: BasicInfo {
            attributes: vfs::ATTR_READONLY | if dir { vfs::ATTR_DIRECTORY } else { 0 },
            ..Default::default()
        },
        end_of_file: len,
        allocation_size: len,
    }
}

impl Backend for Disc {
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        if path.is_empty() || path.eq_ignore_ascii_case("maps") {
            return Ok(meta(0, true));
        }
        if !path.eq_ignore_ascii_case(PATH) {
            return Err(Error::ObjectNameNotFound);
        }
        Ok(meta(self.map.len() as u64, false))
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if !path.eq_ignore_ascii_case(PATH) {
            return Err(Error::ObjectNameNotFound);
        }
        let start = (offset as usize).min(self.map.len());
        let n = (self.map.len() - start).min(buffer.len());
        buffer[..n].copy_from_slice(&self.map[start..start + n]);
        Ok(n)
    }
    fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
        Ok(Vec::new())
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        Err(Error::NotSupported)
    }
}

#[test]
fn a_streamed_rewrite_of_a_200_mib_map_is_the_buffered_build_in_bounded_memory() {
    let original = Arc::new(big_map(200 * MIB));
    assert!(original.len() > 200 * MIB);
    let packages = vec![adds_c()];
    let mut backend = RewriteBackend::new(
        Disc {
            map: Arc::clone(&original),
        },
        PackageRewrite::new(packages.clone()),
    );

    // The rewrite runs at the first stat. Everything it holds or held is
    // what it changes: the header, the tables, the index, the metadata,
    // and the megabyte buffers it reads the map through.
    let before = reset_peak();
    let size = backend.stat(PATH).unwrap().end_of_file;
    let peak = PEAK.load(Ordering::Relaxed) - before;
    let held = LIVE.load(Ordering::Relaxed).saturating_sub(before);
    assert!(peak < 16 * MIB, "the rewrite peaked at {} KiB", peak >> 10);
    assert!(held < 8 * MIB, "the rewrite holds {} KiB", held >> 10);

    // The buffered build, for the bytes.
    let target = Target {
        name: "big",
        scope: MapScope::Campaign,
    };
    let (buffered, report) = build_map(&original, &packages, &target).unwrap();
    assert_eq!(report.added.len(), 1);
    assert_eq!(size, buffered.len() as u64);
    assert!(buffered.len() > original.len(), "the tag data grew");

    let read = |backend: &mut RewriteBackend<Disc, PackageRewrite>, at: u64, len: usize| {
        let mut buf = vec![0xee; len];
        let mut done = 0;
        while done < len {
            match backend.read_at(PATH, at + done as u64, &mut buf[done..]).unwrap() {
                0 => break,
                n => done += n,
            }
        }
        buf.truncate(done);
        buf
    };
    // All of it, in large reads.
    let mut at = 0;
    while at < buffered.len() {
        let got = read(&mut backend, at as u64, 8 * MIB);
        assert!(got == buffered[at..(at + 8 * MIB).min(buffered.len())], "chunk at {at:#x}");
        at += 8 * MIB;
    }
    // Random ranges, with the ends of the file and of the source among them.
    let mut seed = 0x5eed_u64;
    let mut ranges = vec![
        (0, 0x900),
        (original.len() as u64 - 0x100, 0x300),
        (buffered.len() as u64 - 17, 40),
        (buffered.len() as u64, 8),
    ];
    for _ in 0..2000 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let at = (seed >> 16) % (buffered.len() as u64 + 64);
        let len = ((seed >> 4) % 70_000) as usize;
        ranges.push((at, len));
    }
    for (at, len) in ranges {
        let got = read(&mut backend, at, len);
        let from = (at as usize).min(buffered.len());
        let want = &buffered[from..(from + len).min(buffered.len())];
        assert!(got == want, "{len:#x} bytes at {at:#x}");
    }
}
