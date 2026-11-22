//! `h2 live maps import FILE`: turn a third-party mod into a map package
//! that shares no retail bytes (docs/game/maps.md §5, "Importing a mod").
//!
//! Many old mods are whole modded `.map` files, which hold the retail map's
//! bytes and may never be shared. Import reads the retail map the mod is
//! for from the user's own disc (through the VFS's disc backend, so a disc
//! image or a directory of its files), or, for a Multiplayer Map Pack map
//! (containment, warlock, ...), from the user's map pack disc
//! (`crate::live_mappack::from_iso`), writes the difference as a patch
//! layer (`content::patch::diff`: PPF3 when the length is unchanged, which
//! the resolver streams over the disc's map without copying, else BPS),
//! and wraps it as a package over that base: the only blob is the delta.
//! A mod that is already a PPF, IPS or BPS patch is checked against the
//! base and kept as it is. Super PPF and VCDIFF reconstruct the target locally
//! and produce a standard delta over the retail map. A result matching any
//! retail map on either disc is refused.
//!
//! The package's display data comes from a `mod.toml` beside the file
//! (or `--info PATH`), every field optional:
//!
//! ```toml
//! name = "Lockout Snipers"
//! authors = ["who made it"]       # the original author(s): the credit
//! author = "who made it"          # older form of authors, one name
//! release = "Snipers v2"          # the original release's name
//! released = "2005-03"            # YYYY, YYYY-MM or YYYY-MM-DD
//! notes = "..."                   # anything else about the credit
//! description = "..."
//! source_url = "https://..."      # where it came from
//! format = "map"                  # map, ppf, ips, bps, sppf or xdelta
//! base_map = "lockout"            # else --base
//! category = "weapons"            # live_proto::maps::CATEGORIES
//! image = "preview.png"           # beside mod.toml, PNG or JPEG
//! features = ["ai"]               # live_proto::maps::FEATURES, added to the derived ones
//! gametypes = ["ctf", "slayer"]   # live_proto::maps::GAMETYPES; none: any
//! version = "1.2"
//! ```
//!
//! The credit is who made the mod, never the account that publishes it:
//! it goes into the signed document's `credit` (live_proto::maps::Credit)
//! and, as one line, into the manifest's `[package] author` for readers
//! that know only that.
//!
//! The package directory gets `manifest.toml`, the layer, the image and a
//! `mod.toml` with the repository's extras (`category`, `source_url`,
//! `features`, `gametypes`, `version` and the credit), which `h2 live maps
//! publish DIR` reads. `features` is what the mod changes as read off the
//! patched map against its base (`crate::live_mod_features`), with the
//! sidecar's own added.
mod sppf;
mod xdelta;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use content::manifest::{Layer, MANIFEST_FILE, MAX_IMAGE_BYTES, image_signature, is_map_name};
use content::patch::{self, Overlay, Patch};
use content::{Manifest, Sha256};
use live_proto::maps::Credit;
use serde::{Deserialize, Serialize};
use vfs::Backend;

/// Upper bound on a decoded third-party patch before making the Live layer.
const MAX_CONVERTED_MAP: usize = 512 << 20;

/// The sidecar's file name, beside the mod and in the package.
pub const MOD_FILE: &str = "mod.toml";

/// A `mod.toml`. Unknown keys are ignored: sidecars are hand-written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// What the mod changes (`live_proto::maps::FEATURES`): in a sidecar,
    /// added to what import reads off the map; in a package, all of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// The game types it is made for (`live_proto::maps::GAMETYPES`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gametypes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ModInfo {
    /// `path`'s sidecar; None when there is no such file.
    pub fn read(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map(Some)
                .with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| path.display().to_string()),
        }
    }

    /// The credit it gives: `authors` (else `author`), the release, its
    /// date, the source URL and notes, tidied; None when it says nothing.
    pub fn credit(&self) -> Option<Credit> {
        let authors = match &self.authors {
            Some(a) => Credit::authors(a),
            None => Credit::authors(Self::text(&self.author)),
        }
        .authors;
        let c = Credit {
            authors,
            release: self.release.clone(),
            released: self.released.clone(),
            source_url: self.source_url.clone(),
            notes: self.notes.clone(),
        }
        .tidy();
        (!c.is_empty()).then_some(c)
    }

    /// The credit's fields, written back as a package's `mod.toml` keeps
    /// them (`authors`, never the older `author`).
    fn with_credit(mut self, c: Option<&Credit>) -> Self {
        if let Some(c) = c {
            self.authors = (!c.authors.is_empty()).then(|| c.authors.clone());
            self.release = c.release.clone();
            self.released = c.released.clone();
            self.notes = c.notes.clone();
            self.source_url = c.source_url.clone();
        }
        self
    }

    /// Empty strings count as absent.
    fn text(v: &Option<String>) -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
}

/// What the mod file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// A whole modded map.
    Map,
    Ppf,
    Ips,
    Bps,
    Xdelta,
    Sppf,
    Serenity,
}

fn unsupported_serenity() -> anyhow::Error {
    anyhow!(
        "unsupported_serenity: Serenity patches (including v3.3) have no documented delta decoder. \
         Import a locally patched multiplayer map or build."
    )
}

impl InputKind {
    pub fn named(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "map" => Some(Self::Map),
            "ppf" => Some(Self::Ppf),
            "ips" => Some(Self::Ips),
            "bps" => Some(Self::Bps),
            "xdelta" | "xdelta3" | "vcdiff" => Some(Self::Xdelta),
            "sppf" => Some(Self::Sppf),
            "serenity" => Some(Self::Serenity),
            _ => None,
        }
    }

    /// By signature: a map starts with the cache header's `daeh`.
    pub fn sniff(bytes: &[u8]) -> Option<Self> {
        if live_proto::maps::is_whole_map(bytes) {
            Some(Self::Map)
        } else if bytes.starts_with(b"PPF") {
            Some(Self::Ppf)
        } else if bytes.starts_with(b"PATCH") {
            Some(Self::Ips)
        } else if bytes.starts_with(b"BPS1") {
            Some(Self::Bps)
        } else if bytes.starts_with(&[0xd6, 0xc3, 0xc4, 0]) {
            Some(Self::Xdelta)
        } else if bytes.starts_with(&[0x14, 0, 0, 0xf]) {
            Some(Self::Serenity)
        } else {
            None
        }
    }

    /// Serenity's public header describes credits and map names. There is
    /// no public specification for its three methods of rebuilding tags.
    pub fn ensure_supported(self) -> Result<()> {
        if self == Self::Serenity {
            return Err(unsupported_serenity());
        }
        Ok(())
    }

    fn detect(path: &Path, format: Option<&str>, bytes: &[u8]) -> Result<Self> {
        let kind = if let Some(f) = format {
            Self::named(f).ok_or_else(|| anyhow!("unknown patch format {f:?}"))?
        } else {
            // An SPPF has no signature. Honor an identified legacy extension
            // before sniffing, so arbitrary section lengths cannot be magic.
            let legacy = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Self::named)
                .filter(|k| matches!(k, Self::Sppf | Self::Serenity));
            legacy.or_else(|| Self::sniff(bytes)).ok_or_else(|| {
                anyhow!("{}: unrecognised map or patch format (use .sppf for Super PPF)", path.display())
            })?
        };
        kind.ensure_supported()?;
        Ok(kind)
    }
}

/// `patch` applied to `base`, checked against it as the resolver checks.
pub fn apply(base: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
    Ok(match Patch::parse(patch)? {
        Patch::Ppf(ppf) => {
            ppf.check(&mut &base[..])?;
            let mut o = Overlay::new(base.len() as u64);
            o.apply(&ppf.patch)?;
            o.materialise(base.to_vec())
        }
        Patch::Ips(ips) => {
            let mut o = Overlay::new(base.len() as u64);
            o.apply(&ips)?;
            o.materialise(base.to_vec())
        }
        Patch::Bps(bps) => bps.apply(base)?,
    })
}

/// Convert a reconstructed map into the same verified delta as a whole-map
/// import. Only this layer and its manifest are eligible for publication.
fn checked_delta(
    base: &[u8],
    target: &[u8],
    description: &str,
) -> Result<(String, Vec<u8>, Vec<u8>)> {
    if !live_proto::maps::is_whole_map(target) {
        bail!("this is not a map file (no cache header)");
    }
    let w = patch::diff(base, target, description);
    let made = apply(base, &w.bytes).context("checking the patch made")?;
    if made != target {
        bail!("the patch made does not give the mod back");
    }
    Ok((format!("mod.{}", w.format.extension()), w.bytes, made))
}

/// The layer a mod becomes over `base`: its file name and bytes, and the
/// map it makes. A whole map becomes a patch from `base` (checked by
/// applying it); a patch is checked and kept. A mod that changes nothing
/// is refused.
pub fn layer_for(
    kind: InputKind,
    input: &[u8],
    base: &[u8],
    description: &str,
) -> Result<(String, Vec<u8>, Vec<u8>)> {
    let (file, layer, target) = match kind {
        InputKind::Map => checked_delta(base, input, description)?,
        InputKind::Sppf => checked_delta(base, &sppf::apply(base, input)?, description)?,
        InputKind::Xdelta => checked_delta(base, &xdelta::apply(base, input)?, description)?,
        InputKind::Serenity => return Err(unsupported_serenity()),
        InputKind::Ppf | InputKind::Ips | InputKind::Bps => {
            let ext = match kind {
                InputKind::Ppf => "ppf",
                InputKind::Ips => "ips",
                _ => "bps",
            };
            let target = apply(base, input)
                .context("the patch does not apply to your base map: is --base the right map?")?;
            (format!("mod.{ext}"), input.to_vec(), target)
        }
    };
    if target == base {
        bail!("the mod is the same as its base map: there is nothing to share");
    }
    Ok((file, layer, target))
}

/// A whole file from the disc.
fn read_disc_file(disc: &mut impl Backend, path: &str) -> Result<Vec<u8>> {
    let meta = disc
        .stat(path)
        .map_err(|e| anyhow!("{path} on the disc: {e}"))?;
    let len = usize::try_from(meta.end_of_file).context("too large")?;
    let mut out = vec![0; len];
    let mut at = 0;
    while at < out.len() {
        let n = disc
            .read_at(path, at as u64, &mut out[at..])
            .map_err(|e| anyhow!("reading {path} on the disc: {e}"))?;
        if n == 0 {
            bail!("{path} on the disc ended early");
        }
        at += n;
    }
    Ok(out)
}

/// Where a mod's base map was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseFrom {
    /// `maps\<map>.map` on the Halo 2 disc.
    Disc,
    /// The Multiplayer Map Pack disc.
    MapPack,
}

/// The retail map `base` (lower case), from the Halo 2 disc, else from the
/// map pack's maps `pack`.
fn read_base(
    disc: &mut impl Backend,
    pack: &[crate::live_mappack::FoundMap],
    mappack: Option<&Path>,
    base: &str,
) -> Result<(Vec<u8>, BaseFrom)> {
    let path = format!("maps\\{base}.map");
    let why = match disc.stat(&path) {
        Ok(m) if !m.is_directory() => {
            return Ok((read_disc_file(disc, &path)?, BaseFrom::Disc));
        }
        Ok(_) => format!("{path} on the disc is a directory"),
        Err(e) => format!("{path} on the disc: {e}"),
    };
    if let Some(m) = pack.iter().find(|m| m.stem == base) {
        return Ok((m.read()?, BaseFrom::MapPack));
    }
    match mappack {
        Some(iso) => bail!(
            "{why}; the map pack disc {} has no {base}.map either",
            iso.display()
        ),
        None => bail!(
            "{why}; for a Multiplayer Map Pack map give --mappack ISO \
             (or put mappack.iso beside the disc)"
        ),
    }
}

/// The retail map that `bytes` is, if any, on the disc or in the map
/// pack (only maps of the same size are read).
fn retail_twin(
    disc: &mut impl Backend,
    pack: &[crate::live_mappack::FoundMap],
    bytes: &[u8],
) -> Result<Option<String>> {
    let entries = disc
        .read_directory("maps")
        .map_err(|e| anyhow!("listing the disc's maps: {e}"))?;
    let want = Sha256::of(bytes);
    for e in entries {
        if e.metadata.is_directory()
            || e.metadata.end_of_file != bytes.len() as u64
            || !e.name.to_ascii_lowercase().ends_with(".map")
        {
            continue;
        }
        let path = format!("maps\\{}", e.name);
        if Sha256::of(&read_disc_file(&mut *disc, &path)?) == want {
            return Ok(Some(format!("disc's own {}", e.name)));
        }
    }
    for m in pack {
        if m.size as usize == bytes.len() && Sha256::of(&m.read()?) == want {
            return Ok(Some(format!("map pack's {}.map", m.stem)));
        }
    }
    Ok(None)
}

/// The base map and the mod over it, as `import` reads them: `input` (a
/// whole modded map, or a PPF, IPS or BPS; `format` names which, else it
/// is sniffed) over the retail map `base` from the disc at `disc`, or from
/// the map pack disc `mappack`. For `h2 live maps analyze`.
pub fn mod_over_base(
    input: &Path,
    format: Option<&str>,
    disc: &Path,
    mappack: Option<&Path>,
    base: &str,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    mod_bytes_over_base(
        bytes,
        &input.display().to_string(),
        format,
        disc,
        mappack,
        base,
    )
}

/// [`mod_over_base`] of a mod already read (`what` names it in errors).
pub fn mod_bytes_over_base(
    bytes: Vec<u8>,
    what: &str,
    format: Option<&str>,
    disc: &Path,
    mappack: Option<&Path>,
    base: &str,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let input = Path::new(what);
    let kind = InputKind::detect(input, format, &bytes)?;
    let base = base.to_ascii_lowercase();
    if !is_map_name(&base) {
        bail!("{base:?} is not a map name (lockout, midship, ...)");
    }
    let mut d =
        vfs::open_disc(disc).map_err(|e| anyhow!("opening the disc {}: {e}", disc.display()))?;
    let pack = match mappack {
        Some(iso) => crate::live_mappack::from_iso(iso)
            .with_context(|| format!("reading the map pack disc {}", iso.display()))?,
        None => Vec::new(),
    };
    let (base_bytes, _) = read_base(&mut d, &pack, mappack, &base)?;
    let target = match kind {
        InputKind::Map => {
            if !live_proto::maps::is_whole_map(&bytes) {
                bail!("{}: not a map file (no cache header)", input.display());
            }
            bytes
        }
        InputKind::Sppf => sppf::apply(&base_bytes, &bytes)?,
        InputKind::Xdelta => xdelta::apply(&base_bytes, &bytes)?,
        InputKind::Serenity => return Err(unsupported_serenity()),
        _ => apply(&base_bytes, &bytes)
            .context("the patch does not apply to the base map: is --base the right map?")?,
    };
    Ok((base_bytes, target))
}

/// What `h2 live maps import` was asked.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// A modded map or patch over its retail base.
    pub input: PathBuf,
    /// The disc: an image or a directory of its files.
    pub disc: PathBuf,
    /// The Multiplayer Map Pack disc image, for mods of its maps (and so a
    /// mod is never one of its maps). None: Halo 2 disc maps only.
    pub mappack: Option<PathBuf>,
    /// The retail map it is a mod of; else `base_map` in mod.toml.
    pub base: Option<String>,
    /// The package directory; else `<input's stem>.package` beside it.
    pub out: Option<PathBuf>,
    /// The sidecar; else `mod.toml` beside the input.
    pub info: Option<PathBuf>,
    /// Overrides mod.toml's.
    pub name: Option<String>,
    pub category: Option<String>,
}

/// A package written by [`import`].
#[derive(Debug)]
pub struct Imported {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// Which disc the base map is on.
    pub base_from: BaseFrom,
    pub layer_size: u64,
    pub input_size: u64,
}

/// Writes the package for `opts.input` (see the module docs). Reads the
/// disc; uploads nothing.
pub fn import(opts: &ImportOptions) -> Result<Imported> {
    let input_path = &opts.input;
    let input =
        std::fs::read(input_path).with_context(|| format!("reading {}", input_path.display()))?;
    let info_path = opts
        .info
        .clone()
        .unwrap_or_else(|| input_path.parent().unwrap_or(Path::new(".")).join(MOD_FILE));
    let info = ModInfo::read(&info_path)?.unwrap_or_default();
    let kind = InputKind::detect(input_path, ModInfo::text(&info.format).as_deref(), &input)?;
    let base = opts
        .base
        .clone()
        .or_else(|| ModInfo::text(&info.base_map))
        .context("which retail map is this a mod of? Give --base MAP or base_map in mod.toml")?
        .to_ascii_lowercase();
    if !is_map_name(&base) {
        bail!("{base:?} is not a map name (lockout, midship, ...)");
    }
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mod")
        .to_string();
    let name = opts
        .name
        .clone()
        .or_else(|| ModInfo::text(&info.name))
        .unwrap_or_else(|| stem.clone());

    let mut disc = vfs::open_disc(&opts.disc)
        .map_err(|e| anyhow!("opening the disc {}: {e}", opts.disc.display()))?;
    let pack = match &opts.mappack {
        Some(iso) => crate::live_mappack::from_iso(iso)
            .with_context(|| format!("reading the map pack disc {}", iso.display()))?,
        None => Vec::new(),
    };
    let (base_bytes, base_from) = read_base(&mut disc, &pack, opts.mappack.as_deref(), &base)?;
    let (file, layer, target) = layer_for(kind, &input, &base_bytes, &name)?;
    if let Some(twin) = retail_twin(&mut disc, &pack, &target)? {
        bail!("the mod is your {twin}: retail maps are never shared");
    }

    let dir = opts.out.clone().unwrap_or_else(|| {
        input_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!("{stem}.package"))
    });
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let credit = info.credit();
    let mut manifest = Manifest::retail(&base, Sha256::of(&base_bytes));
    manifest.package.name = name.clone();
    manifest.package.author = credit.as_ref().and_then(Credit::names);
    manifest.package.description = ModInfo::text(&info.description);
    manifest.patches = vec![Layer {
        file: file.clone(),
        sha256: Sha256::of(&layer),
    }];
    if let Some(image) = ModInfo::text(&info.image) {
        let from = info_path.parent().unwrap_or(Path::new(".")).join(&image);
        let bytes = std::fs::read(&from)
            .with_context(|| format!("reading the image {}", from.display()))?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            bail!("{}: over {} KiB", from.display(), MAX_IMAGE_BYTES / 1024);
        }
        let ext = image_signature(&bytes)
            .ok_or_else(|| anyhow!("{}: not a PNG or JPEG", from.display()))?;
        let to = format!("preview.{ext}");
        std::fs::write(dir.join(&to), &bytes)?;
        manifest.package.image = Some(to);
    }
    let extras = ModInfo {
        name: Some(name),
        source_url: ModInfo::text(&info.source_url),
        category: opts
            .category
            .clone()
            .or_else(|| ModInfo::text(&info.category)),
        base_map: Some(base),
        features: page_features(&info, &base_bytes, &target),
        gametypes: info.gametypes.clone(),
        version: ModInfo::text(&info.version),
        ..ModInfo::default()
    }
    .with_credit(credit.as_ref());
    let mut doc = live_proto::maps::MapDocument::new(manifest.clone(), None);
    doc.category = extras.category.clone();
    doc.source_url = extras.source_url.clone();
    doc.credit = credit;
    PageExtras::of(&extras).apply(&mut doc);
    doc.validate_extras()
        .map_err(|e| anyhow!("{}: {e}", info_path.display()))?;

    std::fs::write(dir.join(&file), &layer)?;
    std::fs::write(dir.join(MOD_FILE), toml::to_string(&extras)?)?;
    std::fs::write(dir.join(MANIFEST_FILE), manifest.to_toml())?;
    manifest
        .validate_in(&dir)
        .with_context(|| dir.join(MANIFEST_FILE).display().to_string())?;
    Ok(Imported {
        dir,
        manifest,
        base_from,
        layer_size: layer.len() as u64,
        input_size: input.len() as u64,
    })
}

/// The repository extras a package directory's `mod.toml` names, for
/// `publish`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras {
    pub category: Option<String>,
    pub source_url: Option<String>,
    pub credit: Option<Credit>,
}

pub fn extras_in(dir: &Path) -> Result<Extras> {
    let info = ModInfo::read(&dir.join(MOD_FILE))?.unwrap_or_default();
    Ok(Extras {
        category: ModInfo::text(&info.category),
        source_url: ModInfo::text(&info.source_url),
        credit: info.credit(),
    })
}

/// The Mods page's extras a package's `mod.toml` names (docs/game/maps.md
/// §5, "A mod's page"), for `publish`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageExtras {
    pub features: Vec<String>,
    pub gametypes: Vec<String>,
    pub version: Option<String>,
}

impl PageExtras {
    fn of(info: &ModInfo) -> Self {
        PageExtras {
            features: info.features.clone(),
            gametypes: info.gametypes.clone(),
            version: ModInfo::text(&info.version),
        }
    }

    /// Read from `dir/mod.toml` (none there: none).
    pub fn read(dir: &Path) -> Result<Self> {
        Ok(Self::of(
            &ModInfo::read(&dir.join(MOD_FILE))?.unwrap_or_default(),
        ))
    }

    /// Onto the document `publish` signs.
    pub fn apply(self, doc: &mut live_proto::maps::MapDocument) {
        doc.features = self.features;
        doc.gametypes = self.gametypes;
        doc.version = self.version;
    }
}

/// What the mod changes: read off `target` against `base`
/// (`crate::live_mod_features`), with the sidecar's own added, in the
/// checklist's order. A map the reader can't read keeps the sidecar's
/// alone (said on stderr): the import itself does not fail on it.
fn page_features(info: &ModInfo, base: &[u8], target: &[u8]) -> Vec<String> {
    use live_proto::maps::{FEATURES, in_order};
    let mut all = match crate::live_mod_features::derive(base, target) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "import: what the mod changes could not be read ({e:#}); only mod.toml's features are listed"
            );
            Vec::new()
        }
    };
    all.extend(info.features.iter().cloned());
    // An unknown id of the sidecar's is refused by validate_extras, not
    // dropped here.
    let unknown: Vec<String> = info
        .features
        .iter()
        .filter(|f| !FEATURES.contains(&f.as_str()))
        .cloned()
        .collect();
    let mut out = in_order(&all, &FEATURES);
    out.extend(unknown);
    out
}

#[cfg(test)]
#[path = "live_mod_import_tests.rs"]
mod tests;
