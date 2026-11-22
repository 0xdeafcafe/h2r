//! Import an outside release against the owner's copy of each level. Prepare
//! and validate every delta locally, then stage its members and publish one
//! pack. The original maps and archive never enter the upload list.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use live_proto::maps::{ModPackAdvisory, ModPackDocument, mod_pack_id_for};

use crate::live_cmd::{Args, Client, accounts_path, call, pick_account, status_of};
use crate::live_maps::{Signer, publish_member, write_package_request};
use crate::live_mod_import::{ImportOptions, Imported, InputKind, ModInfo, import};

mod source;
mod whole_build;

const USAGE: &str =
    "usage: h2 live mods import DIR_OR_ARCHIVE --pack NAME --author NAME ... --source-url URL
    [--year YYYY] [--cheating-pack | --not-for-online] [--description TEXT] [--version VERSION]
    [--disc ISO_OR_DIR] [--mappack ISO] [--server URL] [--as GAMERTAG]";

struct Candidate {
    logical: PathBuf,
    file: PathBuf,
}

pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("h2-pack-{}", live_proto::sign::random_id()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

// Releases use the retail stem, display name or a level-specific directory,
// e.g. "Relic/starting weapons.ppf" or "Noobs - Beaver Creek.map.ppf".
// A match on the file takes precedence over its parent folders.
fn patch_base(path: &Path, stems: &[String]) -> Result<Option<String>> {
    let mut aliases = BTreeMap::new();
    for stem in stems {
        aliases.insert(words(stem).concat(), stem.as_str());
        if let Some(title) = content::map_pack::title(stem) {
            aliases.insert(words(title).concat(), stem.as_str());
        }
        if stem == "cyclotron" {
            aliases.insert("ivorytower".into(), stem.as_str());
        }
    }
    // Resource and menu patches are commonly bundled with the levels.
    for name in ["mainmenu", "shared", "singleplayershared"] {
        aliases.insert(name.into(), "");
    }
    let leaf = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let folders = path
        .ancestors()
        .skip(1)
        .filter_map(|p| p.file_name()?.to_str());
    for part in std::iter::once(leaf).chain(folders) {
        let tokens = words(part);
        let mut matches = BTreeSet::new();
        for start in 0..tokens.len() {
            let mut key = String::new();
            for token in &tokens[start..] {
                key.push_str(token);
                if let Some(stem) = aliases.get(&key) {
                    matches.insert(*stem);
                }
            }
        }
        match matches.len() {
            0 => continue,
            1 => {
                let stem = matches.into_iter().next().unwrap();
                return Ok((!stem.is_empty()).then(|| stem.to_owned()));
            }
            _ => bail!(
                "{} names more than one base map: {matches:?}",
                path.display()
            ),
        }
    }
    bail!(
        "cannot identify the retail level for {}; use a level-named folder or filename",
        path.display()
    )
}

fn prepare(
    entries: Vec<Candidate>,
    disc: &Path,
    mappack: Option<&Path>,
    scratch: &Path,
    doc: &ModPackDocument,
) -> Result<Vec<Imported>> {
    let retail = whole_build::RetailMaps::open(disc, mappack)?;
    let stems: Vec<String> = retail.stems().map(str::to_owned).collect();
    // A supplied sidecar may refer to arbitrary paths. Build the publication's
    // sidecar exclusively from the explicitly supplied provenance.
    let sidecar = scratch.join("credit.toml");
    let info = ModInfo {
        authors: Some(doc.authors.clone()),
        source_url: doc.source_url.clone(),
        release: Some(doc.name.clone()),
        released: doc.year.map(|year| year.to_string()),
        version: doc.version.clone(),
        ..ModInfo::default()
    };
    fs::write(&sidecar, toml::to_string(&info)?)?;
    let mut packages = BTreeMap::new();
    for entry in entries {
        let ext = entry
            .logical
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if let Some(kind) = InputKind::named(ext) {
            kind.ensure_supported()
                .with_context(|| entry.logical.display().to_string())?;
        }
        let base = if ext.eq_ignore_ascii_case("map") {
            retail.changed_base(&entry.logical, &entry.file)?
        } else {
            patch_base(&entry.logical, &stems)?
        };
        let Some(base) = base else { continue };
        if packages.contains_key(&base) {
            bail!(
                "{}: the release has multiple members for {base}; choose one version per level",
                entry.logical.display()
            );
        }
        let out = scratch.join(format!("package-{base}"));
        let made = import(&ImportOptions {
            input: entry.file,
            disc: disc.to_owned(),
            mappack: mappack.map(Path::to_owned),
            base: Some(base.clone()),
            out: Some(out),
            info: Some(sidecar.clone()),
            name: Some(format!("{}: {base}", doc.name)),
            category: None,
        })
        .with_context(|| format!("converting {}", entry.logical.display()))?;
        packages.insert(base, made);
    }
    if packages.is_empty() {
        bail!("the release contains no changed multiplayer level or recognized patch");
    }
    Ok(packages.into_values().collect())
}

fn required(args: &mut Args, flag: &str) -> Result<String> {
    args.take(flag)?
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("import needs {flag}\n\n{USAGE}"))
}

fn metadata(args: &mut Args) -> Result<ModPackDocument> {
    let name = required(args, "--pack")?;
    let mut authors = vec![required(args, "--author")?];
    while let Some(next) = args.take("--author")? {
        authors.push(next.trim().to_owned());
    }
    let source_url = Some(required(args, "--source-url")?);
    let year = args
        .take("--year")?
        .map(|s| s.parse::<u16>())
        .transpose()
        .context("--year needs a four-digit year")?;
    let advisory = match (args.flag("--cheating-pack"), args.flag("--not-for-online")) {
        (true, true) => bail!("choose one of --cheating-pack and --not-for-online"),
        (true, false) => Some(ModPackAdvisory::CheatingPack),
        (false, true) => Some(ModPackAdvisory::NotForOnline),
        (false, false) => None,
    };
    Ok(ModPackDocument {
        name,
        authors,
        year,
        advisory,
        source_url,
        description: args.take("--description")?,
        version: args.take("--version")?,
        members: Vec::new(),
    })
}

pub(crate) fn run(mut args: Args) -> Result<()> {
    if args.flag("--help") || args.flag("-h") {
        bail!("{USAGE}");
    }
    let mut doc = metadata(&mut args)?;
    let disc = args
        .take("--disc")?
        .map(PathBuf::from)
        .map_or_else(crate::default_disc, Ok)?;
    let mappack = args
        .take("--mappack")?
        .map(PathBuf::from)
        .or_else(|| crate::mappack_cmd::discover(&disc));
    let server = args.take("--server")?;
    let as_tag = args.take("--as")?;
    let [input] = args
        .positional_for(USAGE)?
        .try_into()
        .map_err(|_| anyhow!("import takes one directory or archive\n\n{USAGE}"))?;

    let scratch = Scratch::new()?;
    let entries = source::discover(Path::new(&input), &scratch.0)?;
    let packages = prepare(entries, &disc, mappack.as_deref(), &scratch.0, &doc)?;
    doc.members = packages.iter().map(|p| p.manifest.map_id()).collect();
    doc.validate().map_err(|e| anyhow!(e))?;

    let client = Client::connect(server)?;
    let id = publish_pack(
        &client,
        &accounts_path()?,
        as_tag.as_deref(),
        &doc,
        &packages,
    )?;
    println!("Published {} ({id}), {} maps", doc.name, doc.members.len());
    Ok(())
}

fn publish_pack(
    client: &Client,
    accounts: &Path,
    as_tag: Option<&str>,
    doc: &ModPackDocument,
    packages: &[Imported],
) -> Result<String> {
    let me = pick_account(client, accounts, as_tag)?;
    let id = mod_pack_id_for(me.xuid, &doc.name);
    let body = serde_json::to_vec(doc)?;
    let signer = Signer::Account(me.clone());
    let finalize = || {
        write_package_request(true, || {
            let req = client
                .agent
                .put(&client.url(&format!("/mod-packs/{id}")))
                .set("content-type", "application/json");
            let req =
                client.authorized(req, signer.token(client, live_proto::lxm::CONTENT_WRITE)?)?;
            call(req, Some(&body))
        })
        .with_context(|| format!("publishing pack {id}"))
    };

    // A retry, metadata edit or restoration can already have every member.
    // Restore the pack through its owner's endpoint before attempting to
    // upload a layer withheld by the deletion.
    match finalize() {
        Ok(_) => return Ok(id),
        Err(e) if status_of(&e) == Some(404) => {}
        Err(e) => return Err(e),
    }
    for package in packages {
        let answer = publish_member(client, &me, &package.dir, None, None, Some(&id))?;
        if !answer.missing.is_empty() {
            bail!(
                "{} still has missing layers: {:?}",
                package.manifest.package.name,
                answer.missing
            );
        }
        if answer.map.summary.map_id != package.manifest.map_id() {
            bail!(
                "unexpected MapId returned for {}",
                package.manifest.package.name
            );
        }
    }
    finalize()?;
    Ok(id)
}

#[cfg(test)]
mod tests;
