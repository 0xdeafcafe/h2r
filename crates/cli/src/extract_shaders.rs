//! `h2 extract-shaders`: extract the shader corpus from your own Halo 2 disc
//! image.
//!
//! ```sh
//! cargo run --release -p cli -- extract-shaders --out <out-dir> [--disc <halo2.iso>] [--force]
//! ```
//!
//! The disc defaults like every other `h2` command (`$H2_DISC`, else the
//! first `.iso` in `disc/`).
//!
//! Reads every `.map` on the disc and writes, into `<out-dir>` (use a
//! scratch directory such as `$TMPDIR`; nothing here belongs in the repo):
//!
//! - `<sha256[..16]>.xvu`: each distinct `vrtx` compiled shader (the `'x'`
//!   header plus microcode).
//! - `<sha256[..16]>.psdef`: each distinct `D3DPIXELSHADERDEF`, taken from
//!   offset 6 of a `spas` postprocess implementation.
//! - `manifest.tsv`: file, SHA-256, occurrence count, first occurrence.
//! - `provenance.txt`: disc and map identities (sizes, build strings,
//!   SHA-256 of `default.xbe` and of every byte range read from each map),
//!   the counts `docs/graphics/shaders.md` quotes, and the PSDEF candidate-offset
//!   check.
//!
//! `crates/shaders/tests/corpus.rs` checks a directory against its manifest. Maps are read
//! with `crates/map`.
//!
//! The PSDEF offset check: every implementation (0x132 bytes) is parsed as
//! a PSDEF at each even offset 0..=0x42 and scored twice: against
//! invariants the SDK's encoding implies (not all zero; combiner count
//! 0..=8, 0 meaning texture stages only, with only the documented flag
//! bits; texture modes <= 0x12; dot mappings <= 7; input-texture selectors
//! in their 1- and 2-bit fields; compare modes in 16 bits; active stage
//! output dwords within 20 bits), and on whether it then translates with no
//! warnings. The tool refuses to write a corpus unless offset 6, and only
//! offset 6, passes both for every implementation.

use sha2::{Digest, Sha256};
use shaders::psh::{PSDEF_SIZE, PixelShaderDef};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const USAGE: &str = "usage: h2 extract-shaders --out <dir> [--disc <halo2.iso>] [--force]

Writes every distinct vertex program (*.xvu) and pixel shader definition
(*.psdef) from the disc's maps, plus manifest.tsv and provenance.txt, into
<dir>. Keep <dir> outside the repo: it is game data. Existing files are not
replaced without --force; the disc is never written.";

const HEADER_SIZE: usize = 0x800;
const VRTX_CLASSIFICATION: u32 = 0x1C;
const SPAS_POSTPROCESS: u32 = 0x58;
const SPAS_IMPLEMENTATION: u32 = 0x132;
const PSDEF_OFFSET: usize = 6;

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The pieces of one map this tool reads.
struct Map {
    name: String,
    build: String,
    file_size: u32,
    /// SHA-256 of header + tag index + metadata, the only bytes used.
    digest: String,
    /// Ordinary metadata: virtual base and bytes.
    meta_base: u32,
    meta: Vec<u8>,
    /// (group, name, address) of every tag with a payload.
    tags: Vec<(String, String, u32)>,
}

impl Map {
    /// Opens the map at `base..base + len` of the image with `crates/map`
    /// and loads its ordinary metadata region (where `vrtx` and `spas` live;
    /// they never use BSP address spaces).
    fn read(iso: &Path, base: u64, len: u64, name: &str) -> Result<Self, String> {
        let file = File::open(iso).map_err(|e| e.to_string())?;
        let mut m = map::Map::from_region(file, base, len).map_err(|e| e.to_string())?;
        let err = |e: map::Error| e.to_string();
        let header = m.header().clone();
        let space = m.metadata_space().ok_or("map has no metadata region")?;
        // read_file_range caps single reads, so take big regions in chunks.
        let mut range = |offset: u64, size: u64| -> Result<Vec<u8>, String> {
            const CHUNK: u64 = 32 << 20;
            let mut out = Vec::with_capacity(size as usize);
            let mut at = 0;
            while at < size {
                let n = CHUNK.min(size - at);
                out.extend(m.read_file_range(offset + at, n).map_err(err)?);
                at += n;
            }
            Ok(out)
        };
        let h = range(0, HEADER_SIZE as u64)?;
        let index = range(header.index_offset.into(), header.index_size.into())?;
        let meta = range(space.file_offset, space.size.into())?;
        let tags = m
            .tags()
            .iter()
            .filter_map(|t| {
                let loc = t.location?;
                let name = t.name.clone().unwrap_or_else(|| t.id.to_string());
                Some((t.group.to_string(), name, loc.address))
            })
            .collect();
        let mut digest = Sha256::new();
        digest.update(&h);
        digest.update(&index);
        digest.update(&meta);
        Ok(Self {
            name: name.to_string(),
            build: header.build,
            file_size: header.file_size,
            digest: hex(&digest.finalize()),
            meta_base: space.virtual_base,
            meta,
            tags,
        })
    }

    fn bytes(&self, addr: u32, n: u32) -> Option<&[u8]> {
        let off = addr.checked_sub(self.meta_base)? as usize;
        self.meta.get(off..off.checked_add(n as usize)?)
    }

    fn u32(&self, addr: u32) -> Option<u32> {
        self.bytes(addr, 4).map(|b| le32(b, 0))
    }

    fn u16(&self, addr: u32) -> Option<u16> {
        self.bytes(addr, 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
}

/// Whether `b` (0xF0 bytes) satisfies the PSDEF invariants listed in the
/// module docs.
fn violations(b: &[u8]) -> Vec<&'static str> {
    let Ok(d) = PixelShaderDef::from_bytes(b) else {
        return vec!["short"];
    };
    if b.iter().all(|&x| x == 0) {
        return vec!["all zero"];
    }
    let mut v = Vec::new();
    let count = (d.combiner_count & 0xFF) as usize;
    if count > 8 {
        v.push("combiner count");
    } else if !(0..count).all(|i| d.rgb_outputs[i] >> 20 == 0 && d.alpha_outputs[i] >> 20 == 0) {
        v.push("stage output bits");
    }
    if d.combiner_count & !0x0001_11FF != 0 {
        v.push("combiner flags");
    }
    if d.texture_modes >> 20 != 0 || !(0..4).all(|i| (d.texture_modes >> (5 * i)) & 0x1F <= 0x12) {
        v.push("texture modes");
    }
    if d.dot_mapping >> 12 != 0 || !(0..3).all(|i| (d.dot_mapping >> (4 * i)) & 0xF <= 7) {
        v.push("dot mapping");
    }
    if d.input_texture & !0x0031_0000 != 0 {
        v.push("input texture");
    }
    if d.compare_mode >> 16 != 0 {
        v.push("compare mode");
    }
    v
}

/// Score of one candidate: passes the invariants; also translates without
/// warnings; and does so with at least one general combiner stage (a
/// count of 0 leaves nothing to check, so misaligned sparse data passes).
fn score_psdef(b: &[u8]) -> [bool; 3] {
    if !violations(b).is_empty() {
        return [false; 3];
    }
    let d = PixelShaderDef::from_bytes(b).unwrap();
    let clean = shaders::psh::translate(&d, &Default::default())
        .warnings
        .is_empty();
    [true, clean, clean && d.combiner_count & 0xFF != 0]
}

#[derive(Default)]
struct Corpus {
    /// file name -> (bytes, count, first label)
    files: BTreeMap<String, (Vec<u8>, usize, String)>,
    counts: BTreeMap<String, usize>,
    /// Candidate PSDEF offset -> implementations scoring each of
    /// [`score_psdef`]'s three checks.
    offsets: BTreeMap<usize, [usize; 3]>,
}

impl Corpus {
    fn count(&mut self, key: impl Into<String>, n: usize) {
        *self.counts.entry(key.into()).or_insert(0) += n;
    }

    fn add(&mut self, ext: &str, bytes: &[u8], label: String) {
        let name = format!("{}.{ext}", &sha256(bytes)[..16]);
        let e = self
            .files
            .entry(name)
            .or_insert_with(|| (bytes.to_vec(), 0, label));
        e.1 += 1;
    }

    fn scan(&mut self, m: &Map) {
        self.count("maps", 1);
        for (group, name, addr) in &m.tags {
            let label = |i: String| format!("{}:{name}:{i}", m.name);
            match group.as_str() {
                "vrtx" => {
                    self.count("vrtx tags", 1);
                    let (Some(n), Some(p)) = (m.u32(addr + 4), m.u32(addr + 8)) else {
                        self.count("unresolved pointers", 1);
                        continue;
                    };
                    if let Some(platform) = m.u16(*addr) {
                        self.count(format!("vrtx platform {platform}"), 1);
                    }
                    for i in 0..n {
                        let e = p + i * VRTX_CLASSIFICATION;
                        self.count("vrtx classifications", 1);
                        let (Some(size), Some(ptr), Some(code)) =
                            (m.u32(e + 0xC), m.u32(e + 0x10), m.u32(e + 0x14))
                        else {
                            self.count("unresolved pointers", 1);
                            continue;
                        };
                        if code != 0 {
                            self.count("vrtx classifications with source code", 1);
                        }
                        if size == 0 {
                            self.count("vrtx empty classifications", 1);
                            continue;
                        }
                        let Some(blob) = m.bytes(ptr, size) else {
                            self.count("unresolved pointers", 1);
                            continue;
                        };
                        self.count(
                            format!("vrtx header {}", hex(&blob[..2.min(blob.len())])),
                            1,
                        );
                        // Is the u16 block the set of v registers read?
                        let list = match (m.u32(e + 4), m.u32(e + 8)) {
                            (Some(n), Some(p)) => m.bytes(p, 2 * n).map(|b| {
                                b.as_chunks::<2>().0.iter().fold(0u32, |acc, c| {
                                    acc | 1u32
                                        .checked_shl(u16::from_le_bytes([c[0], c[1]]) as u32)
                                        .unwrap_or(0)
                                })
                            }),
                            _ => None,
                        };
                        if let (Some(list), Ok(p)) = (list, shaders::vsh::Program::parse(blob)) {
                            let same = list == p.inputs_read() as u32;
                            self.count(format!("vrtx u16 block equals inputs read: {same}"), 1);
                        }
                        self.add("xvu", blob, label(i.to_string()));
                    }
                }
                "spas" => {
                    self.count("spas tags", 1);
                    let (Some(n), Some(p)) = (m.u32(addr + 0x1C), m.u32(addr + 0x20)) else {
                        self.count("unresolved pointers", 1);
                        continue;
                    };
                    for i in 0..n {
                        let pp = p + i * SPAS_POSTPROCESS;
                        self.count("spas postprocess entries", 1);
                        let (Some(icnt), Some(iptr)) = (m.u32(pp), m.u32(pp + 4)) else {
                            self.count("unresolved pointers", 1);
                            continue;
                        };
                        for j in 0..icnt {
                            let Some(imp) =
                                m.bytes(iptr + j * SPAS_IMPLEMENTATION, SPAS_IMPLEMENTATION)
                            else {
                                self.count("unresolved pointers", 1);
                                continue;
                            };
                            self.count("spas implementations", 1);
                            if imp[PSDEF_OFFSET + 0xD4] != 0 {
                                self.count("spas implementations with combiner stages", 1);
                            }
                            for off in (0..=SPAS_IMPLEMENTATION as usize - PSDEF_SIZE).step_by(2) {
                                let s = score_psdef(&imp[off..off + PSDEF_SIZE]);
                                let e = self.offsets.entry(off).or_insert([0; 3]);
                                for k in 0..3 {
                                    e[k] += s[k] as usize;
                                }
                            }
                            let def = &imp[PSDEF_OFFSET..PSDEF_OFFSET + PSDEF_SIZE];
                            for why in violations(def) {
                                self.count(format!("psdef at {PSDEF_OFFSET:#x} fails: {why}"), 1);
                            }
                            self.add("psdef", def, label(format!("{i}:{j}")));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Write one output file. Existing files are kept unless `force`; the disc
/// being read, anything holding an XDVDFS volume and `.iso`/`.xiso` names are
/// never written, even with `force`. (Same rules as `h2`'s `safe_out`; this
/// example cannot depend on the cli binary crate.)
fn guarded_write(dir: &Path, name: &str, bytes: &[u8], disc: &Path, force: bool) -> Result<(), String> {
    use std::io::Write;
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name == ".." || name == "." {
        return Err(format!("unsafe output name {name:?}"));
    }
    let dest = dir.join(name);
    let ext = dest.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("iso" | "xiso")) {
        return Err(format!("refusing to write {}: looks like a disc image", dest.display()));
    }
    if let Ok(m) = std::fs::symlink_metadata(&dest) {
        if !force {
            return Err(format!("{} exists (use --force to replace it)", dest.display()));
        }
        let same = match (dest.canonicalize(), disc.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        #[cfg(unix)]
        let same = same || {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(disc).is_ok_and(|d| d.dev() == m.dev() && d.ino() == m.ino())
        };
        if same {
            return Err(format!("refusing to write {}: it is the disc being read", dest.display()));
        }
        if !m.is_file() {
            return Err(format!("{} is not a regular file", dest.display()));
        }
        // Fail closed: content that cannot be inspected is not replaced.
        match std::fs::File::open(&dest) {
            Ok(f) => {
                if xiso::Xiso::open(f).is_ok() {
                    return Err(format!("refusing to write {}: it is an Xbox disc image", dest.display()));
                }
            }
            Err(e) => {
                return Err(format!("refusing to replace {}: cannot inspect it ({e})", dest.display()));
            }
        }
        std::fs::remove_file(&dest).map_err(|e| e.to_string())?;
    }
    // create_new never follows a symlink planted at the name.
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&dest).map_err(|e| e.to_string())?;
    f.write_all(bytes).map_err(|e| e.to_string())
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut out, mut disc, mut force) = (None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().context("--out needs a directory")?)),
            "--disc" => disc = Some(PathBuf::from(args.next().context("--disc needs an ISO path")?)),
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => bail!("unexpected argument {a}\n{USAGE}"),
        }
    }
    let Some(out) = out else { bail!("--out <dir> is required\n{USAGE}") };
    let iso_path = match disc {
        Some(d) => d,
        None => super::default_disc()?,
    };
    extract(&iso_path, &out, force).map_err(anyhow::Error::msg)
}

fn extract(iso_path: &Path, out: &Path, force: bool) -> Result<(), String> {
    let iso_len = std::fs::metadata(iso_path)
        .map_err(|e| e.to_string())?
        .len();
    let mut x = xiso::Xiso::open(File::open(iso_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let entries = x.entries().map_err(|e| e.to_string())?;
    let mut prov = String::new();
    let _ = writeln!(prov, "tool\tcli/src/extract_shaders.rs");
    let _ = writeln!(prov, "iso_size\t{iso_len}");
    let _ = writeln!(prov, "iso_partition_offset\t{:#x}", x.partition_offset());
    if let Some(xbe) = entries
        .iter()
        .find(|e| e.path.eq_ignore_ascii_case("/default.xbe"))
    {
        let bytes = x.read(xbe).map_err(|e| e.to_string())?;
        let _ = writeln!(prov, "default_xbe\t{}\t{}", bytes.len(), sha256(&bytes));
    }

    let mut corpus = Corpus::default();
    let mut maps: Vec<_> = entries
        .iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        .collect();
    maps.sort_by(|a, b| a.path.cmp(&b.path));
    for e in maps {
        let base = x.partition_offset() + e.sector as u64 * xiso::SECTOR;
        let name = e.path.rsplit('/').next().unwrap().trim_end_matches(".map");
        match Map::read(iso_path, base, e.size as u64, name) {
            Ok(m) => {
                let _ = writeln!(
                    prov,
                    "map\t{}\t{}\t{}\t{}\t{}",
                    e.path, e.size, m.file_size, m.build, m.digest
                );
                corpus.scan(&m);
            }
            Err(err) => {
                let _ = writeln!(prov, "map_skipped\t{}\t{err}", e.path);
            }
        }
    }

    let (xvu, psdef) = corpus.files.keys().fold((0, 0), |(v, p), k| {
        if k.ends_with(".xvu") {
            (v + 1, p)
        } else {
            (v, p + 1)
        }
    });
    corpus.count("unique xvu", xvu);
    corpus.count("unique psdef", psdef);
    // Facts about the distinct definitions that docs/graphics/shaders.md quotes.
    let defs: Vec<PixelShaderDef> = corpus
        .files
        .iter()
        .filter(|(k, _)| k.ends_with(".psdef"))
        .map(|(_, (b, _, _))| PixelShaderDef::from_bytes(b).unwrap())
        .collect();
    for d in &defs {
        let c = d.decode();
        corpus.count(
            format!("psdef combiner flags {:#x}", d.combiner_count >> 8),
            1,
        );
        corpus.count(format!("psdef combiner stages {}", c.stages.len()), 1);
        if d.c0_mapping != 0 || d.c1_mapping != 0 || d.final_constants != 0 {
            corpus.count("psdef with constant mappings", 1);
        }
        if d.constant0.iter().any(|&w| w != 0) {
            corpus.count("psdef with embedded c0", 1);
        }
        for (i, m) in c.texture_modes.iter().enumerate() {
            corpus.count(format!("psdef texture mode {}", m.name()), 1);
            if i > 0 && m.is_dot() {
                corpus.count(format!("psdef dot mapping {:?}", c.dot_mapping[i]), 1);
            }
        }
    }
    for (k, v) in &corpus.counts {
        let _ = writeln!(prov, "count\t{k}\t{v}");
    }

    // The PSDEF offset must be unambiguous before anything is written.
    let total = corpus
        .counts
        .get("spas implementations")
        .copied()
        .unwrap_or(0);
    let _ = writeln!(
        prov,
        "psdef_offset_check\timplementations\t{total}\t(offset: passing invariants, also translating cleanly, and with combiner stages)"
    );
    for (off, [n, clean, staged]) in &corpus.offsets {
        if *n > 0 {
            let _ = writeln!(
                prov,
                "psdef_offset_candidate\t{off:#x}\t{n}\t{clean}\t{staged}"
            );
        }
    }
    let full: Vec<usize> = corpus
        .offsets
        .iter()
        .filter(|(_, [n, clean, _])| *n == total && *clean == total)
        .map(|(o, _)| *o)
        .collect();
    if total == 0 || full != [PSDEF_OFFSET] {
        eprint!("{prov}");
        return Err(format!(
            "PSDEF offset check failed: offsets passing for all {total} implementations: {full:?}"
        ));
    }

    write_outputs(out, &corpus.files, &mut prov, iso_path, force)?;
    print!("{prov}");
    Ok(())
}

/// Write the corpus, `manifest.tsv` and `provenance.txt` into `out`, all
/// through [`guarded_write`].
fn write_outputs(
    out: &Path,
    files: &BTreeMap<String, (Vec<u8>, usize, String)>,
    prov: &mut String,
    iso_path: &Path,
    force: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut manifest = String::new();
    for (name, (bytes, n, label)) in files {
        guarded_write(out, name, bytes, iso_path, force)?;
        let _ = writeln!(manifest, "{name}\t{}\t{n}\t{label}", sha256(bytes));
    }
    guarded_write(out, "manifest.tsv", manifest.as_bytes(), iso_path, force)?;
    let _ = writeln!(prov, "manifest_sha256\t{}", sha256(manifest.as_bytes()));
    guarded_write(out, "provenance.txt", prov.as_bytes(), iso_path, force)?;
    Ok(())
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    fn scratch(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-corpus-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn corpus_writes_never_clobber_or_touch_the_disc() {
        let d = scratch("guard");
        let disc = d.join("disc.bin");
        std::fs::write(&disc, b"disc").unwrap();
        // Source disc: refused even with force.
        assert!(guarded_write(&d, "disc.bin", b"x", &disc, true).is_err());
        assert_eq!(std::fs::read(&disc).unwrap(), b"disc");
        // Existing file: refused without force, replaced with it.
        std::fs::write(d.join("a.xvu"), b"old").unwrap();
        assert!(guarded_write(&d, "a.xvu", b"new", &disc, false).is_err());
        assert_eq!(std::fs::read(d.join("a.xvu")).unwrap(), b"old");
        guarded_write(&d, "a.xvu", b"new", &disc, true).unwrap();
        assert_eq!(std::fs::read(d.join("a.xvu")).unwrap(), b"new");
        // Disc names and disc content: refused.
        assert!(guarded_write(&d, "x.iso", b"x", &disc, true).is_err());
        let mut img = vec![0u8; 0x10000];
        img.extend_from_slice(b"MICROSOFT*XBOX*MEDIA");
        img.extend_from_slice(&[0u8; 8]);
        std::fs::write(d.join("img.dat"), &img).unwrap();
        assert!(guarded_write(&d, "img.dat", b"x", &disc, true).is_err());
        assert_eq!(std::fs::read(d.join("img.dat")).unwrap(), img);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&disc, d.join("l.xvu")).unwrap();
            assert!(guarded_write(&d, "l.xvu", b"x", &disc, true).is_err());
            assert_eq!(std::fs::read(&disc).unwrap(), b"disc");
        }
    }

    fn corpus() -> BTreeMap<String, (Vec<u8>, usize, String)> {
        BTreeMap::from([("a.xvu".to_string(), (b"shader".to_vec(), 1usize, "label".to_string()))])
    }

    /// Each of the three real write sites must be guarded: pre-existing
    /// files survive a run without `--force`, and the disc never changes.
    #[test]
    fn write_outputs_guards_every_call_site() {
        for existing in ["a.xvu", "manifest.tsv", "provenance.txt"] {
            let d = scratch(&format!("sites-{existing}"));
            let disc = d.join("disc.bin");
            std::fs::write(&disc, b"disc").unwrap();
            std::fs::write(d.join(existing), b"mine").unwrap();
            let mut prov = String::new();
            assert!(write_outputs(&d, &corpus(), &mut prov, &disc, false).is_err(), "{existing}");
            assert_eq!(std::fs::read(d.join(existing)).unwrap(), b"mine", "{existing} was clobbered");
            // With --force it is replaced, and the disc is still untouched.
            let mut prov = String::new();
            write_outputs(&d, &corpus(), &mut prov, &disc, true).unwrap();
            assert_ne!(std::fs::read(d.join(existing)).unwrap(), b"mine");
            assert_eq!(std::fs::read(&disc).unwrap(), b"disc");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_images_are_never_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("unreadable");
        let disc = d.join("disc.bin");
        std::fs::write(&disc, b"disc").unwrap();
        let mut img = vec![0u8; 0x10000];
        img.extend_from_slice(b"MICROSOFT*XBOX*MEDIA");
        img.extend_from_slice(&[0u8; 8]);
        let victim = d.join("provenance.txt");
        std::fs::write(&victim, &img).unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&victim).is_ok() {
            return; // root
        }
        let mut prov = String::new();
        assert!(write_outputs(&d, &corpus(), &mut prov, &disc, true).is_err());
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), img);
    }
}
