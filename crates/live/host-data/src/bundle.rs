//! Build the owner's bundle from the DVD filesystem. The light host boots
//! the original XBE and retail menu, including its packed fonts, and uses
//! the shared caches and multiplayer maps for matches.

mod stores;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufReader, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use map::CacheType;
use xiso::Xiso;

use crate::{tar::TarWriter, Error, FileEntry, HashWriter, Manifest, ARCHIVE_NAME, MANIFEST_NAME};

/// The cache header, in bytes (`HEADER_SIZE`, crates/formats/map/src/reader.rs).
const HEADER: usize = 0x800;
/// Word offsets inside the header, matching crates/formats/map/src/reader.rs.
const OFF_VERSION: usize = 4;
const OFF_CACHE_TYPE: usize = 0x140;
const OFF_FOOTER: usize = 0x7fc;

/// Everything [`build`] needs: the disc, where the bundle goes, and the
/// optional directory of stripped server maps.
pub struct BundleSpec {
    /// The owner's disc: an XISO image (XDVDFS at any known partition
    /// offset) or a directory holding the disc's files.
    pub disc: PathBuf,
    /// Directory the two bundle files are written to (created if needed).
    pub out: PathBuf,
    /// Directory of stripped server maps: a `<name>.map` here replaces
    /// the disc's `maps/<name>.map`.
    pub stripped: Option<PathBuf>,
    /// Directory of previously recorded shared resource stores to verify against the disc.
    pub sim_stores: Option<PathBuf>,
    /// Replace existing bundle files.
    pub force: bool,
    /// zstd compression level, 1..=21.
    pub zstd_level: i32,
}

/// What [`build`] produced, for the command's summary and the tests.
#[derive(Debug)]
pub struct BundleOut {
    /// The manifest as written beside the archive, with its sha256.
    pub manifest: Manifest,
    /// Multiplayer maps included.
    pub multiplayer: usize,
    /// Shared maps included (main menu, shared, single-player shared).
    pub shared: usize,
    /// Campaign maps found and excluded.
    pub campaign_skipped: usize,
    /// Maps with an unrecognized cache type, excluded with a warning.
    pub unknown_skipped: usize,
    /// Maps taken from the stripped directory instead of the disc.
    pub stripped_used: usize,
    /// Number of shared resource maps carried as stores.
    pub stores_used: usize,
    /// Payload bytes before compression.
    pub raw_bytes: u64,
    /// Bytes of the written archive.
    pub archive_bytes: u64,
}

/// Build the bundle: `host-data.tar.zst` and `host-data.json` in
/// `spec.out`.
pub fn build(spec: &BundleSpec) -> Result<BundleOut, Error> {
    let bundle = Error::Bundle;
    if !(1..=21).contains(&spec.zstd_level) {
        return Err(bundle(format!("zstd level {} is outside 1..=21", spec.zstd_level)));
    }
    let mut disc = Disc::open(&spec.disc)?;

    // default.xbe, whole: the module stamp is a hash of the entire XBE.
    let xbe = disc.read_xbe()?;
    if xbe.len() < 4 || xbe[..4] != *b"XBEH" {
        return Err(bundle("default.xbe on the disc is not an XBE image".into()));
    }

    // Every maps/*.map, classified by its cache header.
    let stripped_index = index_stripped(spec.stripped.as_deref())?;
    let mut payload: Vec<Payload> = vec![Payload {
        path: "default.xbe".into(),
        size: xbe.len() as u64,
        source: Source::Bytes(xbe),
        sha256: String::new(),
    }];
    let (mut multiplayer, mut shared, mut campaign_skipped, mut unknown_skipped, mut stripped_used) =
        (0, 0, 0, 0, 0);
    for map in disc.files(&["maps"])? {
        if !map.name.to_ascii_lowercase().ends_with(".map") {
            continue;
        }
        let header = read_header(&map.source, map.size, &map.name)?;
        let kind = classify(&map.name, &header)?;
        match kind {
            CacheType::Campaign => {
                campaign_skipped += 1;
                continue;
            }
            CacheType::Unknown(ty) => {
                eprintln!("host-data: maps/{} has cache type {ty}; leaving it out", map.name);
                unknown_skipped += 1;
                continue;
            }
            CacheType::Multiplayer => multiplayer += 1,
            _ => shared += 1,
        }
        let path = format!("maps/{}", map.name);
        let (source, size) = match stripped_index.get(&map.name.to_ascii_lowercase()) {
            Some(stripped) => {
                let metadata = fs::symlink_metadata(stripped)?;
                if !metadata.is_file() {
                    return Err(bundle(format!("{} must be a regular file", stripped.display())));
                }
                let size = metadata.len();
                let source = Source::File(stripped.clone());
                if size != map.size || read_header(&source, size, &map.name)? != header {
                    return Err(bundle(format!(
                        "{} must match the original map header and length",
                        stripped.display()
                    )));
                }
                stripped_used += 1;
                (source, size)
            }
            None => (map.source, map.size),
        };
        payload.push(Payload { path, size, source, sha256: String::new() });
    }
    for font in disc.files(&["maps", "fonts"])? {
        payload.push(Payload {
            path: format!("maps/fonts/{}", font.name),
            size: font.size,
            source: font.source,
            sha256: String::new(),
        });
    }
    payload.sort_by(|a, b| a.path.cmp(&b.path));
    let mut seen = BTreeSet::new();
    for file in &payload {
        crate::validate_payload_path(&file.path)?;
        if !seen.insert(file.path.to_ascii_lowercase()) {
            return Err(bundle(format!("the disc has two files named {}", file.path)));
        }
    }
    for path in crate::REQUIRED_FILES {
        if !seen.contains(path) {
            return Err(bundle(format!("no {path} on the disc")));
        }
    }

    // Output checks, then write.
    let tar_path = spec.out.join(ARCHIVE_NAME);
    let json_path = spec.out.join(MANIFEST_NAME);
    for existing in [&tar_path, &json_path] {
        match fs::symlink_metadata(existing) {
            Ok(metadata) if spec.force && metadata.is_file() => {}
            Ok(_) => {
                return Err(bundle(format!(
                    "{} exists; --force can replace only regular files",
                    existing.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if let Disc::Dir { root } = &disc {
        refuse_overlap(root, &spec.out)?;
    }
    crate::private_dir(&spec.out, true)?;

    let staging = spec.out.join(format!(".host-data-{}", crate::unique_suffix()));
    crate::private_dir(&staging, false)?;
    let result = (|| -> Result<_, Error> {
        let stores_used = if let Some(stores) = &spec.sim_stores {
            stores::stage(stores, &spec.disc, &staging, &stripped_index, &mut payload)?
        } else {
            0
        };
        let archive = staging.join(ARCHIVE_NAME);
        let metadata = staging.join(MANIFEST_NAME);
        let manifest = write_archive(&archive, spec.zstd_level, &mut payload)?;
        let archive_bytes = fs::metadata(&archive)?.len();
        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| bundle(format!("writing {MANIFEST_NAME}: {e}")))?;
        use std::io::Write;
        let mut manifest_file = crate::new_private_file(&metadata)?;
        manifest_file.write_all(&json)?;
        manifest_file.sync_all()?;
        drop(manifest_file);

        if spec.force {
            fs::rename(&archive, &tar_path)?;
            fs::rename(&metadata, &json_path)?;
        } else {
            fs::hard_link(&archive, &tar_path)?;
            if let Err(error) = fs::hard_link(&metadata, &json_path) {
                let _ = fs::remove_file(&tar_path);
                return Err(error.into());
            }
        }
        Ok((manifest, archive_bytes, stores_used))
    })();
    let _ = fs::remove_dir_all(staging);
    let (manifest, archive_bytes, stores_used) = result?;
    let raw_bytes = manifest.files.iter().map(|file| file.size).sum();

    Ok(BundleOut {
        manifest,
        multiplayer,
        shared,
        campaign_skipped,
        unknown_skipped,
        stripped_used,
        stores_used,
        raw_bytes,
        archive_bytes,
    })
}

/// One file on the way into the archive.
struct Payload {
    path: String,
    size: u64,
    source: Source,
    /// Filled by [`write_archive`].
    sha256: String,
}

/// Where a payload file's bytes come from.
enum Source {
    /// A bounded region of the ISO file: (iso path, absolute byte
    /// offset, length).
    IsoRegion(PathBuf, u64, u64),
    /// A whole file on this machine (DVD-directory disc, stripped maps).
    File(PathBuf),
    /// Bytes already in memory (default.xbe).
    Bytes(Vec<u8>),
}

impl Source {
    /// A fresh reader over the source. Cheap for paths; the in-memory
    /// variant copies (default.xbe, once).
    fn open(&self) -> Result<Box<dyn Read>, std::io::Error> {
        match self {
            Self::IsoRegion(iso, offset, len) => {
                let mut f = File::open(iso)?;
                f.seek(SeekFrom::Start(*offset))?;
                Ok(Box::new(f.take(*len)))
            }
            Self::File(p) => Ok(Box::new(File::open(p)?)),
            Self::Bytes(b) => Ok(Box::new(Cursor::new(b.clone()))),
        }
    }
}

/// A direct file in the selected disc directory.
struct DiscFile {
    /// File name as the disc spells it (`lockout.map`).
    name: String,
    size: u64,
    source: Source,
}

/// The disc: an XISO image or a directory of the disc's files.
enum Disc {
    Iso { iso: Xiso<BufReader<File>>, path: PathBuf },
    Dir { root: PathBuf },
}

impl Disc {
    fn open(path: &Path) -> Result<Self, Error> {
        let bundle = Error::Bundle;
        let md = fs::metadata(path).map_err(|e| bundle(format!("{}: {e}", path.display())))?;
        if md.is_dir() {
            return Ok(Self::Dir { root: path.to_path_buf() });
        }
        if !md.is_file() {
            return Err(bundle(format!("{} is neither an ISO nor a directory", path.display())));
        }
        let file = File::open(path).map_err(|e| bundle(format!("{}: {e}", path.display())))?;
        match Xiso::open(BufReader::new(file)) {
            Ok(iso) => Ok(Self::Iso { iso, path: path.to_path_buf() }),
            Err(xiso::Error::NotXdvdfs) => {
                // A bare XBE is a fair mistake: h2 build accepts one.
                let mut magic = [0u8; 4];
                let is_xbe = File::open(path)
                    .and_then(|mut f| f.read_exact(&mut magic))
                    .is_ok()
                    && magic == *b"XBEH";
                let hint = if is_xbe { " (it is an XBE; the bundle needs the whole disc)" } else { "" };
                Err(bundle(format!("{} is not an XDVDFS image{hint}", path.display())))
            }
            Err(e) => Err(bundle(format!("{}: {e}", path.display()))),
        }
    }

    /// The disc's default.xbe.
    fn read_xbe(&mut self) -> Result<Vec<u8>, Error> {
        match self {
            Self::Iso { iso, .. } => {
                let entry = iso
                    .find("default.xbe")
                    .map_err(|_| Error::Bundle("no default.xbe on the disc".into()))?;
                if entry.is_dir {
                    return Err(Error::Bundle("default.xbe must be a file".into()));
                }
                Ok(iso.read(&entry)?)
            }
            Self::Dir { root } => {
                let xbe = find_in_dir(root, "default.xbe")?
                    .ok_or_else(|| Error::Bundle(format!("no default.xbe in {}", root.display())))?;
                if !fs::symlink_metadata(&xbe)?.is_file() {
                    return Err(Error::Bundle(format!("{} must be a regular file", xbe.display())));
                }
                fs::read(&xbe).map_err(|e| Error::Bundle(format!("{}: {e}", xbe.display())))
            }
        }
    }

    /// Regular files directly under a selected DVD directory.
    fn files(&mut self, components: &[&str]) -> Result<Vec<DiscFile>, Error> {
        match self {
            Self::Iso { iso, path } => {
                let partition = iso.partition_offset();
                let dir = components.join("/");
                let mut out = Vec::new();
                for entry in iso.entries()? {
                    let Some((parent, name)) = entry.path.trim_start_matches('/').rsplit_once('/') else {
                        continue;
                    };
                    if entry.is_dir || !parent.eq_ignore_ascii_case(&dir) {
                        continue;
                    }
                    let offset = partition + u64::from(entry.sector) * xiso::SECTOR;
                    let size = u64::from(entry.size);
                    out.push(DiscFile {
                        name: name.to_owned(),
                        size,
                        source: Source::IsoRegion(path.clone(), offset, size),
                    });
                }
                Ok(out)
            }
            Self::Dir { root } => {
                let mut dir = root.clone();
                for name in components {
                    let Some(child) = find_in_dir(&dir, name)? else {
                        return Ok(Vec::new());
                    };
                    if !fs::symlink_metadata(&child)?.is_dir() {
                        return Err(Error::Bundle(format!("{} must be a directory", child.display())));
                    }
                    dir = child;
                }
                let mut out = Vec::new();
                for entry in fs::read_dir(&dir)? {
                    let entry = entry?;
                    let metadata = fs::symlink_metadata(entry.path())?;
                    if metadata.is_dir() {
                        continue;
                    }
                    if !metadata.is_file() {
                        return Err(Error::Bundle(format!(
                            "{} must be a regular file",
                            entry.path().display()
                        )));
                    }
                    let name = entry.file_name().into_string()
                        .map_err(|_| Error::Bundle("non-UTF-8 file name on the disc".into()))?;
                    out.push(DiscFile {
                        name,
                        size: metadata.len(),
                        source: Source::File(entry.path()),
                    });
                }
                Ok(out)
            }
        }
    }
}

/// A case-insensitive DVD name resolves to at most one child.
fn find_in_dir(dir: &Path, want: &str) -> Result<Option<PathBuf>, Error> {
    let mut found = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name().eq_ignore_ascii_case(want) && found.replace(entry.path()).is_some() {
            return Err(Error::Bundle(format!("{} has more than one {want}", dir.display())));
        }
    }
    Ok(found)
}

/// The stripped directory indexed by lower-case file name.
fn index_stripped(dir: Option<&Path>) -> Result<BTreeMap<String, PathBuf>, Error> {
    let Some(dir) = dir else { return Ok(BTreeMap::new()) };
    let mut out = BTreeMap::new();
    let entries = fs::read_dir(dir).map_err(|e| Error::Bundle(format!("{}: {e}", dir.display())))?;
    for e in entries {
        let e = e.map_err(|e| Error::Bundle(format!("{}: {e}", dir.display())))?;
        let Ok(name) = e.file_name().into_string() else { continue };
        if name.to_ascii_lowercase().ends_with(".map")
            && out.insert(name.to_ascii_lowercase(), e.path()).is_some() {
                return Err(Error::Bundle(format!("{} has more than one {name}", dir.display())));
            }
    }
    Ok(out)
}

/// Read one map's cache header.
fn read_header(source: &Source, size: u64, name: &str) -> Result<[u8; HEADER], Error> {
    if size < HEADER as u64 {
        return Err(Error::Bundle(format!(
            "maps/{name} is {size} bytes, smaller than a cache header"
        )));
    }
    let mut buf = [0u8; HEADER];
    let mut reader = source.open().map_err(|e| Error::Bundle(format!("maps/{name}: {e}")))?;
    reader
        .read_exact(&mut buf)
        .map_err(|e| Error::Bundle(format!("reading maps/{name}: {e}")))?;
    Ok(buf)
}

fn classify(name: &str, header: &[u8; HEADER]) -> Result<CacheType, Error> {
    let bad = |why: &str| Error::Bundle(format!("maps/{name}: {why}"));
    if &header[..4] != b"daeh" || &header[OFF_FOOTER..] != b"toof" {
        return Err(bad("not a Halo 2 cache file (head/foot signature)"));
    }
    let version = u32::from_le_bytes(header[OFF_VERSION..OFF_VERSION + 4].try_into().unwrap());
    if version != 8 {
        return Err(bad(&format!("cache version {version}, wanted 8")));
    }
    let ty = u32::from_le_bytes(header[OFF_CACHE_TYPE..OFF_CACHE_TYPE + 4].try_into().unwrap());
    Ok(ty.into())
}

/// A DVD-directory disc is read while the bundle is written; writing the
/// bundle inside it mixes the output with the disc files.
fn refuse_overlap(disc: &Path, out: &Path) -> Result<(), Error> {
    let disc_path = disc.canonicalize()?;
    let absolute_out = std::path::absolute(out)?;
    // Resolve the nearest existing ancestor even when --out is new.
    let mut base = absolute_out.as_path();
    let resolved = loop {
        match base.canonicalize() {
            Ok(path) => break path.join(absolute_out.strip_prefix(base).unwrap()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                base = base.parent().ok_or(error)?;
            }
            Err(error) => return Err(error.into()),
        }
    };
    if resolved.starts_with(&disc_path) {
        return Err(Error::Bundle(format!(
            "--out {} overlaps the disc directory {}",
            out.display(),
            disc.display()
        )));
    }
    Ok(())
}

/// Stream every payload into the zstd-compressed tar at `temp`, then the
/// manifest last (it carries the payload digests). Fills in each
/// payload's sha256 and returns the completed manifest.
fn write_archive(temp: &Path, level: i32, payload: &mut [Payload]) -> Result<Manifest, Error> {
    let bundle = Error::Bundle;
    let file = crate::new_private_file(temp)?;
    let hashed = HashWriter::new(file);
    let mut encoder = zstd::stream::write::Encoder::new(hashed, level)?;
    let mut manifest = {
        let mut tar = TarWriter::new(&mut encoder);
        for p in payload.iter_mut() {
            let mut src = p.source.open().map_err(|e| bundle(format!("{}: {e}", p.path)))?;
            p.sha256 = tar
                .append(&p.path, p.size, &mut src)
                .map_err(|e| bundle(format!("{}: {e}", p.path)))?;
        }
        // The embedded manifest names every payload; its own sha256 field
        // is empty because it cannot contain the archive's digest.
        let embedded = Manifest {
            sha256: String::new(),
            files: payload
                .iter()
                .map(|p| FileEntry {
                    path: p.path.clone(),
                    size: p.size,
                    sha256: p.sha256.clone(),
                })
                .collect(),
        };
        let json = serde_json::to_vec(&embedded)
            .map_err(|e| bundle(format!("writing {MANIFEST_NAME}: {e}")))?;
        if json.len() as u64 > crate::MAX_MANIFEST_BYTES {
            return Err(bundle(format!("{MANIFEST_NAME} is too large")));
        }
        tar.append(MANIFEST_NAME, json.len() as u64, &mut Cursor::new(json))
            .map_err(|e| bundle(format!("{MANIFEST_NAME}: {e}")))?;
        tar.finish().map_err(|e| bundle(format!("finishing the archive: {e}")))?;
        embedded
    };
    let (file, digest) = encoder
        .finish()
        .map_err(|e| bundle(format!("finishing compression: {e}")))?
        .into_parts();
    // The archive is complete on disk before it is published by rename.
    file.sync_all()?;
    manifest.sha256 = digest;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{self, TempDir};

    fn spec(disc: &Path, out: &Path) -> BundleSpec {
        BundleSpec {
            disc: disc.to_path_buf(),
            out: out.to_path_buf(),
            stripped: None,
            sim_stores: None,
            force: false,
            zstd_level: 3,
        }
    }

    fn read_manifest(out: &Path) -> Manifest {
        serde_json::from_slice(&testkit::read_all(&out.join(MANIFEST_NAME))).unwrap()
    }

    #[test]
    fn the_manifest_names_the_archive_and_every_payload_file() {
        let tmp = TempDir::new("manifest");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        testkit::write_disc_dir(&disc);
        let out = tmp.join("bundle");
        let built = build(&spec(&disc, &out)).unwrap();

        let manifest = read_manifest(&out);
        assert_eq!(manifest.sha256, built.manifest.sha256);
        // The archive on disk really hashes to what the manifest says.
        assert_eq!(
            manifest.sha256,
            testkit::sha256_hex(&testkit::read_all(&out.join(ARCHIVE_NAME)))
        );
        // Payload paths are sorted and cover boot assets and multiplayer maps.
        let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "default.xbe",
                "maps/fonts/conduit-12",
                "maps/fonts/font_table.txt",
                "maps/lockout.map",
                "maps/mainmenu.map",
                "maps/shared.map",
                "maps/single_player_shared.map",
                "maps/zanzibar.map",
            ]
        );
        // One file's size and digest, checked against the disc original.
        let lockout = manifest.files.iter().find(|f| f.path == "maps/lockout.map").unwrap();
        let original = testkit::read_all(&disc.join("maps/lockout.map"));
        assert_eq!(lockout.size, original.len() as u64);
        assert_eq!(lockout.sha256, testkit::sha256_hex(&original));
        // The campaign map never made it in.
        assert!(!manifest.files.iter().any(|f| f.path.contains("a30")));
        assert_eq!(built.campaign_skipped, 1);
    }

    #[test]
    fn a_disc_image_yields_the_same_bundle_as_a_dvd_directory() {
        let tmp = TempDir::new("iso");
        let disc = tmp.join("disc");
        testkit::write_disc_dir(&disc);
        let iso = tmp.join("disc.iso");
        testkit::write_disc_image(&iso, &disc);

        let from_dir = build(&spec(&disc, &tmp.join("dir-bundle"))).unwrap();
        let from_iso = build(&spec(&iso, &tmp.join("iso-bundle"))).unwrap();
        assert_eq!(from_iso.manifest, from_dir.manifest);
    }

    #[test]
    fn a_stripped_map_replaces_the_discs_copy() {
        let tmp = TempDir::new("stripped");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        testkit::write_disc_dir(&disc);
        let stripped = tmp.join("stripped");
        std::fs::create_dir_all(&stripped).unwrap();
        let mut stripped_lockout = testkit::read_all(&disc.join("maps/lockout.map"));
        stripped_lockout[HEADER..].fill(0);
        std::fs::write(stripped.join("lockout.map"), &stripped_lockout).unwrap();

        let mut s = spec(&disc, &tmp.join("bundle"));
        s.stripped = Some(stripped);
        let built = build(&s).unwrap();
        assert_eq!(built.stripped_used, 1);

        let manifest = read_manifest(&tmp.join("bundle"));
        let lockout = manifest.files.iter().find(|f| f.path == "maps/lockout.map").unwrap();
        assert_eq!(lockout.sha256, testkit::sha256_hex(&stripped_lockout));
        assert_eq!(lockout.size, stripped_lockout.len() as u64);
        // The untouched map still comes from the disc.
        let zanzibar = manifest.files.iter().find(|f| f.path == "maps/zanzibar.map").unwrap();
        let original = testkit::read_all(&disc.join("maps/zanzibar.map"));
        assert_eq!(zanzibar.sha256, testkit::sha256_hex(&original));
    }

    #[test]
    fn a_stripped_map_from_a_different_disc_is_rejected() {
        let tmp = TempDir::new("mismatched-map");
        let disc = tmp.join("disc");
        testkit::write_disc_dir(&disc);
        let stripped = tmp.join("stripped");
        fs::create_dir(&stripped).unwrap();
        let mut wrong = testkit::read_all(&disc.join("maps/lockout.map"));
        wrong[0x120] ^= 1;
        fs::write(stripped.join("lockout.map"), wrong).unwrap();

        let mut spec = spec(&disc, &tmp.join("bundle"));
        spec.stripped = Some(stripped);
        let error = build(&spec).unwrap_err();
        assert!(error.to_string().contains("must match the original map header and length"));
    }

    #[test]
    fn an_existing_bundle_is_not_replaced_without_force() {
        let tmp = TempDir::new("force");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        testkit::write_disc_dir(&disc);
        let out = tmp.join("bundle");
        build(&spec(&disc, &out)).unwrap();
        let err = build(&spec(&disc, &out)).unwrap_err();
        assert!(err.to_string().contains("--force"), "{err}");
        let mut s = spec(&disc, &out);
        s.force = true;
        build(&s).unwrap();
    }

    #[test]
    fn a_disc_directory_is_never_its_own_output() {
        let tmp = TempDir::new("overlap");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        testkit::write_disc_dir(&disc);
        let inside = disc.join("bundle");
        let err = build(&spec(&disc, &inside)).unwrap_err();
        assert!(err.to_string().contains("overlaps"), "{err}");
        let mut s = spec(&disc, &disc);
        s.force = true;
        assert!(build(&s).is_err());
    }

    #[test]
    fn a_broken_map_header_fails_the_build_with_the_maps_name() {
        let tmp = TempDir::new("brokenmap");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        std::fs::write(disc.join("default.xbe"), b"XBEH ok").unwrap();
        std::fs::write(disc.join("maps/mainmenu.map"), testkit::map_bytes(2, "mainmenu")).unwrap();
        std::fs::write(disc.join("maps/broken.map"), b"not a map at all").unwrap();
        let err = build(&spec(&disc, &tmp.join("bundle"))).unwrap_err();
        assert!(err.to_string().contains("maps/broken.map"), "{err}");
    }

    #[test]
    fn an_unknown_cache_type_is_skipped_with_a_warning() {
        let tmp = TempDir::new("unknowntype");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        testkit::write_disc_dir(&disc);
        std::fs::write(
            disc.join("maps/oddity.map"),
            testkit::map_bytes(9, "oddity"),
        )
        .unwrap();
        let built = build(&spec(&disc, &tmp.join("bundle"))).unwrap();
        assert_eq!(built.unknown_skipped, 1);
        let manifest = read_manifest(&tmp.join("bundle"));
        assert!(!manifest.files.iter().any(|f| f.path.contains("oddity")));
    }

    #[test]
    fn a_disc_without_default_xbe_is_refused() {
        let tmp = TempDir::new("noxbe");
        let disc = tmp.join("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        std::fs::write(disc.join("maps/mainmenu.map"), testkit::map_bytes(2, "mainmenu")).unwrap();
        let err = build(&spec(&disc, &tmp.join("bundle"))).unwrap_err();
        assert!(err.to_string().contains("default.xbe"), "{err}");
    }

    #[test]
    fn a_missing_disc_is_refused() {
        let tmp = TempDir::new("nodisc");
        let err = build(&spec(&tmp.join("nowhere"), &tmp.join("bundle"))).unwrap_err();
        assert!(err.to_string().contains("nowhere"), "{err}");
    }
}
