//! `h2 live maps import FILE`: turn a third-party mod into a map package
//! that shares no retail bytes (docs/game/maps.md §5, "Importing a mod").
//!
//! Many old mods are whole modded `.map` files, which hold the retail map's
//! bytes and may never be shared. Import reads the retail map the mod is
//! for from the user's own disc (through the VFS's disc backend, so a disc
//! image or a directory of its files), writes the difference as a patch
//! layer (`content::patch::diff`: PPF3 when the length is unchanged, which
//! the resolver streams over the disc's map without copying, else BPS),
//! and wraps it as a package over that base: the only blob is the delta.
//! A mod that is already a PPF, IPS or BPS patch is checked against the
//! base and kept as it is.
//!
//! The package's display data comes from a `mod.toml` beside the file
//! (or `--info PATH`), every field optional:
//!
//! ```toml
//! name = "Lockout Snipers"
//! author = "who made it"          # [package] author: the credit
//! description = "..."
//! source_url = "https://..."      # where it came from
//! format = "map"                  # map, ppf, ips or bps; else sniffed
//! base_map = "lockout"            # else --base
//! category = "weapons"            # live_proto::maps::CATEGORIES
//! image = "preview.png"           # beside mod.toml, PNG or JPEG
//! ```
//!
//! The package directory gets `manifest.toml`, the layer, the image and a
//! `mod.toml` with the repository's extras (`category`, `source_url`),
//! which `h2 live maps publish DIR` reads.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use content::manifest::{Layer, MANIFEST_FILE, MAX_IMAGE_BYTES, image_signature, is_map_name};
use content::patch::{self, Overlay, Patch};
use content::{Manifest, Sha256};
use serde::{Deserialize, Serialize};
use vfs::Backend;

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
}

impl InputKind {
    pub fn named(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "map" => Some(Self::Map),
            "ppf" => Some(Self::Ppf),
            "ips" => Some(Self::Ips),
            "bps" => Some(Self::Bps),
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
        } else {
            None
        }
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
        InputKind::Map => {
            if !live_proto::maps::is_whole_map(input) {
                bail!("this is not a map file (no cache header)");
            }
            let w = patch::diff(base, input, description);
            let made = apply(base, &w.bytes).context("checking the patch made")?;
            if made != input {
                bail!("the patch made does not give the mod back");
            }
            (format!("mod.{}", w.format.extension()), w.bytes, made)
        }
        InputKind::Ppf | InputKind::Ips | InputKind::Bps => {
            let ext = match kind {
                InputKind::Ppf => "ppf",
                InputKind::Ips => "ips",
                _ => "bps",
            };
            let target = apply(base, input)
                .context("the patch does not apply to your disc's map: is --base the right map?")?;
            (format!("mod.{ext}"), input.to_vec(), target)
        }
    };
    if target == base {
        bail!("the mod is the same as your disc's map: there is nothing to share");
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

/// The disc map that `bytes` is, if any (only maps of the same size are
/// read).
fn retail_twin(disc: &mut impl Backend, bytes: &[u8]) -> Result<Option<String>> {
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
        if Sha256::of(&read_disc_file(disc, &path)?) == want {
            return Ok(Some(e.name));
        }
    }
    Ok(None)
}

/// What `h2 live maps import` was asked.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// The mod: a `.map`, `.ppf`, `.ips` or `.bps`.
    pub input: PathBuf,
    /// The disc: an image or a directory of its files.
    pub disc: PathBuf,
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
    let kind = match ModInfo::text(&info.format) {
        Some(f) => InputKind::named(&f).ok_or_else(|| {
            anyhow!(
                "{}: format {f:?} is not map, ppf, ips or bps",
                info_path.display()
            )
        })?,
        None => InputKind::sniff(&input).ok_or_else(|| {
            anyhow!(
                "{}: not a map, PPF, IPS or BPS file (give format in {MOD_FILE})",
                input_path.display()
            )
        })?,
    };
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
    let base_bytes = read_disc_file(&mut disc, &format!("maps\\{base}.map"))?;
    let (file, layer, target) = layer_for(kind, &input, &base_bytes, &name)?;
    if let Some(twin) = retail_twin(&mut disc, &target)? {
        bail!("the mod is your disc's own {twin}: retail maps are never shared");
    }

    let dir = opts.out.clone().unwrap_or_else(|| {
        input_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!("{stem}.package"))
    });
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut manifest = Manifest::retail(&base, Sha256::of(&base_bytes));
    manifest.package.name = name.clone();
    manifest.package.author = ModInfo::text(&info.author);
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
        ..ModInfo::default()
    };
    let mut doc = live_proto::maps::MapDocument::new(manifest.clone(), None);
    doc.category = extras.category.clone();
    doc.source_url = extras.source_url.clone();
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
        layer_size: layer.len() as u64,
        input_size: input.len() as u64,
    })
}

/// The repository extras a package directory's `mod.toml` names, for
/// `publish`.
pub fn extras_in(dir: &Path) -> Result<(Option<String>, Option<String>)> {
    let info = ModInfo::read(&dir.join(MOD_FILE))?.unwrap_or_default();
    Ok((
        ModInfo::text(&info.category),
        ModInfo::text(&info.source_url),
    ))
}

#[cfg(test)]
#[path = "live_mod_import_tests.rs"]
mod tests;
