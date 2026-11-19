//! `h2 live maps details` and `h2 live maps analyze`: an admin's details
//! of published packages (docs/game/maps.md §5, "Details";
//! live_proto::maps::Details) without touching what their publishers
//! signed, and the contents report that goes in them.
//!
//! `analyze FILE --base MAP` reads a mod (a modded `.map`, or a PPF, IPS
//! or BPS) over the user's own disc map, as `import` does, and prints what
//! it contains ([`crate::live_mod_contents`]); `--out FILE.toml` writes
//! the report for `details --contents`. `analyze --map-id ID` with no
//! file reads a published package instead: its manifest and its one patch
//! layer from the server (no token, so it counts no download), over its
//! base from the disc.
//!
//! `details --file DETAILS.toml` sets many packages' details and the map
//! packs' info from one file (docs/game/mod-details.toml):
//!
//! ```toml
//! [[pack]]
//! id = "ai-revision"                # a-z, 0-9 and -
//! name = "Halo 2 Remapped AI Revision"
//! description = """..."""
//! authors = ["Zone 117"]
//! released = "2007-06"
//! source_url = "https://..."
//! [[pack.image]]                    # files in IMAGES/pack-<id>/
//! file = "01-menu.jpg"
//! caption = "..."
//! source_url = "https://..."        # the page it was found on
//! credit = "who made it"
//! licence = "..."
//!
//! [[mod]]
//! map_id = "..."                    # checked against the server
//! name = "Zombiebar"                # likewise, when given
//! slug = "air-zombiebar"            # IMAGES/<slug>/, CONTENTS/<slug>.toml
//! pack = "ai-revision"
//! description = """..."""
//! features = ["ai", "scripts"]      # optional: replaces the listed ones
//! [[mod.image]]                     # as a pack's
//! ```
//!
//! Other keys (`image_url`, `description_sources`, notes) are for the
//! reader. Images are PNG or JPEG of at most 512 KiB, uploaded as content
//! by an account (`--as`); with the operator's key (`--data`) they must
//! already be stored, or `--as` names the account that uploads them.
//! Each package and pack is compared with the server's first and left
//! alone when it already says so; `--dry-run` says what would change and
//! sends nothing.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use content::manifest::MAX_IMAGE_BYTES;
use content::{MapId, Sha256};
use live_proto::lxm;
use live_proto::maps::{Details, MapAnswer, MapImage, MapSummary, PackInfo, PackListed};

use crate::live_cmd::{Client, call};
use crate::live_maps::{Signer, list};
use crate::live_mod_contents::Analysis;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct ImageRow {
    pub file: String,
    #[serde(default)]
    pub caption: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub credit: Option<String>,
    #[serde(default)]
    pub licence: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct PackRow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub released: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default, rename = "image")]
    pub images: Vec<ImageRow>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct ModRow {
    pub map_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub pack: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub features: Option<Vec<String>>,
    #[serde(default, rename = "image")]
    pub images: Vec<ImageRow>,
}

/// A slug names a directory: 1 to 64 of `a-z`, `0-9`, `_` and `-`.
fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A details file.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct DetailsFile {
    #[serde(default, rename = "pack")]
    pub packs: Vec<PackRow>,
    #[serde(default, rename = "mod")]
    pub mods: Vec<ModRow>,
}

impl DetailsFile {
    pub fn parse(text: &str) -> Result<Self> {
        let f: DetailsFile = toml::from_str(text)?;
        let mut ids = std::collections::BTreeSet::new();
        for p in &f.packs {
            if !live_proto::maps::pack_id_ok(&p.id) {
                bail!("pack {:?}: an id is a-z, 0-9 and -", p.id);
            }
            if !ids.insert(p.id.clone()) {
                bail!("pack {} is listed twice", p.id);
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut slugs = std::collections::BTreeSet::new();
        for m in &f.mods {
            let id: MapId = m
                .map_id
                .parse()
                .map_err(|_| anyhow!("{:?} is not a MapId", m.map_id))?;
            if !seen.insert(id) {
                bail!("{id} is listed twice");
            }
            if let Some(s) = &m.slug
                && (!slug_ok(s) || !slugs.insert(s.clone()))
            {
                bail!("{id}: slug {s:?} is not a-z, 0-9, _ and -, or is used twice");
            }
            if !m.images.is_empty() && m.slug.is_none() {
                bail!("{id}: images need a slug (their directory)");
            }
        }
        Ok(f)
    }
}

/// One image read from disk: its blob and how it is listed.
#[derive(Debug, Clone)]
pub(crate) struct Shot {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub listed: MapImage,
}

fn read_shots(dir: Option<&Path>, sub: &str, rows: &[ImageRow]) -> Result<Vec<Shot>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let dir = dir.context("the file lists images: give --images DIR")?;
    rows.iter()
        .map(|r| {
            let path = dir.join(sub).join(&r.file);
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            if live_proto::maps::image_type(&bytes).is_none() {
                bail!("{}: not a PNG or JPEG", path.display());
            }
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                bail!("{}: over {} KiB", path.display(), MAX_IMAGE_BYTES / 1024);
            }
            Ok(Shot {
                listed: MapImage {
                    sha256: Sha256::of(&bytes),
                    caption: r.caption.clone(),
                    source_url: r.source_url.clone(),
                    credit: r.credit.clone(),
                    licence: r.licence.clone(),
                },
                path,
                bytes,
            })
        })
        .collect()
}

/// What one `[[mod]]` sets.
#[derive(Debug, Clone)]
pub(crate) struct Wanted {
    pub map_id: MapId,
    pub name: Option<String>,
    pub details: Details,
    pub shots: Vec<Shot>,
}

/// What one `[[pack]]` sets.
#[derive(Debug, Clone)]
pub(crate) struct WantedPack {
    pub id: String,
    pub info: PackInfo,
    pub shots: Vec<Shot>,
}

/// The file's packages and packs with their images read from `images`
/// and their reports from `contents` (`<slug>.toml`, when there).
pub(crate) fn wanted(
    file: &DetailsFile,
    images: Option<&Path>,
    contents: Option<&Path>,
) -> Result<(Vec<WantedPack>, Vec<Wanted>)> {
    let mut packs = Vec::new();
    for p in &file.packs {
        let shots = read_shots(images, &format!("pack-{}", p.id), &p.images)?;
        let info = PackInfo {
            name: p.name.clone(),
            description: p.description.clone(),
            authors: p.authors.clone(),
            released: p.released.clone(),
            source_url: p.source_url.clone(),
            images: shots.iter().map(|s| s.listed.clone()).collect(),
        }
        .tidy();
        info.validate().map_err(|e| anyhow!("pack {}: {e}", p.id))?;
        packs.push(WantedPack {
            id: p.id.clone(),
            info,
            shots,
        });
    }
    let mut mods = Vec::new();
    for m in &file.mods {
        let map_id: MapId = m.map_id.parse().expect("checked when parsed");
        let slug = m.slug.as_deref().unwrap_or("");
        let shots = read_shots(images, slug, &m.images)?;
        let report = match (contents, &m.slug) {
            (Some(dir), Some(slug)) => {
                let path = dir.join(format!("{slug}.toml"));
                if path.exists() {
                    let text = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    let a: Analysis =
                        toml::from_str(&text).with_context(|| path.display().to_string())?;
                    if let Some(id) = &a.map_id
                        && id != &m.map_id
                    {
                        bail!("{}: for {id}, not {}", path.display(), m.map_id);
                    }
                    Some(a.contents.bounded())
                } else {
                    None
                }
            }
            _ => None,
        };
        let features = m
            .features
            .as_ref()
            .map(|f| live_proto::maps::in_order(f, &live_proto::maps::FEATURES));
        let details = Details {
            description: m.description.clone(),
            images: shots.iter().map(|s| s.listed.clone()).collect(),
            contents: report,
            features,
            pack: m.pack.clone(),
        }
        .tidy();
        details.validate().map_err(|e| anyhow!("{map_id}: {e}"))?;
        mods.push(Wanted {
            map_id,
            name: m.name.clone(),
            details,
            shots,
        });
    }
    Ok((packs, mods))
}

/// Puts each image's blob (an account uploads; with the operator's key
/// it must already be stored). Says nothing when it is all there.
fn store_shots(
    client: &Client,
    uploader: Option<&Signer>,
    shots: &[&Shot],
    dry_run: bool,
    out: &mut Vec<String>,
) -> Result<()> {
    for s in shots {
        let sha = s.listed.sha256;
        let have = client
            .agent
            .get(&client.url(&format!("/content/{sha}")))
            .call()
            .is_ok();
        if have {
            continue;
        }
        match uploader {
            _ if dry_run => out.push(format!("  would upload {} ({sha})", s.path.display())),
            Some(u) if matches!(u, Signer::Account(_)) => {
                let req = client
                    .agent
                    .put(&client.url(&format!("/content/{sha}")))
                    .set("content-type", "application/octet-stream");
                let req = client.authorized(req, u.token(client, lxm::CONTENT_WRITE)?)?;
                call(req, Some(&s.bytes))
                    .with_context(|| format!("uploading {}", s.path.display()))?;
                out.push(format!("  uploaded {} ({sha})", s.path.display()));
            }
            _ => bail!(
                "{} is not on the server yet: images are uploaded by an account, give --as GAMERTAG",
                s.path.display()
            ),
        }
    }
    Ok(())
}

/// Whether the server already says what `w` sets.
fn already(w: &Details, m: &MapSummary, report: Option<&live_proto::maps::Contents>) -> bool {
    w.description
        .as_ref()
        .is_none_or(|d| m.description.as_deref() == Some(d.as_str()))
        && w.images == m.images
        && w.contents.as_ref() == report
        && w.features.as_ref().is_none_or(|f| f == &m.features)
        && w.pack == m.pack
}

/// Sets each pack's info and each package's details that differ from the
/// server's, uploading their images first; or with `dry_run` only says
/// what it would. One line per pack and package. A package the server
/// does not list, or names otherwise, is an error before anything is sent.
pub(crate) fn apply_details(
    client: &Client,
    admin: &Signer,
    uploader: Option<&Signer>,
    packs: &[WantedPack],
    mods: &[Wanted],
    dry_run: bool,
) -> Result<Vec<String>> {
    let listed: HashMap<MapId, MapSummary> = list(client, None, None)?
        .into_iter()
        .map(|m| (m.map_id, m))
        .collect();
    for w in mods {
        let m = listed
            .get(&w.map_id)
            .with_context(|| format!("{}: the server lists no such package", w.map_id))?;
        if let Some(name) = &w.name
            && name.trim() != m.name
        {
            bail!(
                "{}: the server names it {:?}, the file {:?}",
                w.map_id,
                m.name,
                name
            );
        }
    }
    let mut out = Vec::new();
    #[derive(serde::Deserialize)]
    struct Packs {
        items: Vec<PackListed>,
    }
    let have: Packs = client.get("/mappacks", &[])?;
    for p in packs {
        let now = have.items.iter().find(|h| h.id == p.id).map(|h| &h.info);
        if now == Some(&p.info) {
            out.push(format!("pack {}: already so", p.id));
            continue;
        }
        let shots: Vec<&Shot> = p.shots.iter().collect();
        store_shots(client, uploader, &shots, dry_run, &mut out)?;
        if dry_run {
            out.push(format!(
                "pack {}: would set {:?} ({} images)",
                p.id,
                p.info.name,
                p.info.images.len()
            ));
            continue;
        }
        let req = client
            .agent
            .put(&client.url(&format!("/mappacks/{}", p.id)))
            .set("content-type", "application/json");
        let req = client.authorized(req, admin.token(client, lxm::CONTENT_MODERATE)?)?;
        call(req, Some(&serde_json::to_vec(&p.info)?))
            .with_context(|| format!("setting pack {}", p.id))?;
        out.push(format!("pack {}: set", p.id));
    }
    for w in mods {
        let m = &listed[&w.map_id];
        let answer: MapAnswer = client.get(&format!("/maps/{}", w.map_id), &[])?;
        if already(&w.details, m, answer.map.report.as_ref()) {
            out.push(format!("{}  {}: already so", w.map_id, m.name));
            continue;
        }
        let shots: Vec<&Shot> = w.shots.iter().collect();
        store_shots(client, uploader, &shots, dry_run, &mut out)?;
        let say = format!(
            "{} description, {} images, {} report lines{}",
            if w.details.description.is_some() {
                "a"
            } else {
                "no"
            },
            w.details.images.len(),
            w.details.contents.as_ref().map_or(0, |c| c.lines.len()),
            w.details
                .pack
                .as_ref()
                .map(|p| format!(", pack {p}"))
                .unwrap_or_default()
        );
        if dry_run {
            out.push(format!("{}  {}: would set {say}", w.map_id, m.name));
            continue;
        }
        let req = client
            .agent
            .put(&client.url(&format!("/maps/{}/details", w.map_id)))
            .set("content-type", "application/json");
        let req = client.authorized(req, admin.token(client, lxm::CONTENT_MODERATE)?)?;
        call(req, Some(&serde_json::to_vec(&w.details)?))
            .with_context(|| format!("setting the details of {} ({})", m.name, w.map_id))?;
        out.push(format!("{}  {}: set {say}", w.map_id, m.name));
    }
    Ok(out)
}

/// `analyze`'s inputs.
#[derive(Debug, Clone, Default)]
pub(crate) struct AnalyzeOptions {
    /// The mod file; None: the published package `map_id`'s layer.
    pub input: Option<PathBuf>,
    pub disc: PathBuf,
    pub mappack: Option<PathBuf>,
    /// The base map; None: the package's (with no file only).
    pub base: Option<String>,
    pub format: Option<String>,
    pub map_id: Option<String>,
}

/// A published package's base map and its one patch layer's bytes.
pub(crate) fn published_layer(client: &Client, id: &MapId) -> Result<(String, Vec<u8>)> {
    let answer: MapAnswer = client.get(&format!("/maps/{id}"), &[])?;
    let m = &answer.map.manifest;
    if m.base.derived {
        bail!("{id} is built on another package: analyze its file with --base");
    }
    let [layer] = &m.patches[..] else {
        bail!(
            "{id} has {} patch layers and {} packs: analyze its file with --base",
            m.patches.len(),
            m.packs.len()
        );
    };
    if !m.packs.is_empty() {
        bail!("{id} has packs: analyze its file with --base");
    }
    let resp = client
        .agent
        .get(&client.url(&format!("/content/{}", layer.sha256)))
        .call()
        .with_context(|| format!("fetching {id}'s layer {}", layer.sha256))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)?;
    if Sha256::of(&bytes) != layer.sha256 {
        bail!("{id}'s layer did not come back as {}", layer.sha256);
    }
    Ok((m.base.map.clone(), bytes))
}

/// The report of `opts.input` (or the published package's layer) over
/// its base from the disc.
pub(crate) fn analyze(opts: &AnalyzeOptions, client: Option<&Client>) -> Result<Analysis> {
    let id = match &opts.map_id {
        Some(id) => Some(
            id.parse::<MapId>()
                .map_err(|_| anyhow!("{id:?} is not a MapId"))?,
        ),
        None => None,
    };
    let (base_name, base, modded) = match &opts.input {
        Some(input) => {
            let base_name = opts.base.clone().context("analyze FILE needs --base MAP")?;
            let (b, m) = crate::live_mod_import::mod_over_base(
                input,
                opts.format.as_deref(),
                &opts.disc,
                opts.mappack.as_deref(),
                &base_name,
            )?;
            (base_name, b, m)
        }
        None => {
            let id = id.context("analyze takes a mod file, or --map-id ID of a published one")?;
            let client = client.context("no server to read the package from")?;
            let (base_name, layer) = published_layer(client, &id)?;
            if let Some(b) = &opts.base
                && !b.eq_ignore_ascii_case(&base_name)
            {
                bail!("{id} is a mod of {base_name}, not {b}");
            }
            let (b, m) = crate::live_mod_import::mod_bytes_over_base(
                layer,
                &format!("{id}'s layer"),
                None,
                &opts.disc,
                opts.mappack.as_deref(),
                &base_name,
            )?;
            (base_name, b, m)
        }
    };
    let (features, contents) = crate::live_mod_contents::analyze(&base, &modded)?;
    Ok(Analysis {
        map_id: opts.map_id.clone(),
        base: Some(base_name),
        features,
        contents,
    })
}

#[cfg(test)]
#[path = "live_maps_details_tests.rs"]
mod tests;
