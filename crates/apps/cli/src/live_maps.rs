//! `h2 live maps publish|import|list`: share a map package over Live
//! (docs/game/maps.md §5, docs/live/live-api.md "Custom maps").
//!
//! `publish DIR` reads the package's `manifest.toml` (checked as the
//! runtime checks it), uploads every layer and the preview image to the
//! content store by SHA-256, then signs the manifest as a `MapPublished`
//! event with the local account's key and puts it; the repository's
//! extras (category, source URL) come from the package's `mod.toml` or
//! the flags. `import FILE` turns a third-party mod (a whole modded map,
//! or a PPF, IPS or BPS) into a package over the user's own disc map
//! (crate::live_mod_import) and publishes it. `list` prints what the
//! server has. `credit` sets who made published packages (an admin's
//! `PUT /v1/maps/{id}/credit`), one by flags or many from a credits file
//! (docs/game/mod-credits.toml). `analyze` reports what a mod contains
//! and `details` sets published packages' descriptions, screenshots,
//! reports and map packs (crate::live_maps_details,
//! docs/game/mod-details.toml).
use std::path::Path;

use anyhow::{Context, Result, bail};
use content::manifest::{MANIFEST_FILE, relative};
use content::{Manifest, MapId, Sha256};
use live_proto::maps::{Credit, MAP_PUBLISHED, MapAnswer, MapDocument, MapSummary, package_stream};
use live_proto::sign::{EventBody, issue_token};
use live_proto::{Page, lxm};

use crate::live_accounts;
use crate::live_cmd::{Args, Client, accounts_path, call, now, pick_account};
use crate::live_mod_import::{ImportOptions, extras_in, import};

pub const USAGE: &str = "usage: h2 live maps <command>

  publish DIR [--server URL] [--as GAMERTAG] [--category C] [--source-url URL]
                          upload a map package (DIR/manifest.toml, its
                          layers and image) and publish it as yours
  import FILE [--base MAP] [--disc ISO] [--mappack ISO] [--out DIR]
              [--name NAME] [--info mod.toml] [--category C] [--no-publish]
              [--server URL] [--as GAMERTAG]
                          make a package of a third-party mod (a modded
                          .map, or a .ppf/.ips/.bps) over your own disc's
                          map, sharing only the difference, and publish it;
                          a Map Pack map's base is read from --mappack,
                          else a mappack.iso beside the disc
  list [--server URL] [--base MAP] [--author GAMERTAG] [--json]
                          the packages the server has
  credit MAP_ID [--author NAME]... [--release NAME] [--released DATE]
                [--source-url URL] [--notes TEXT] [--clear]
  credit --file CREDITS.toml [--dry-run]
                [--server URL] [--as GAMERTAG | --data DIR]
                          set who made published packages (an admin's
                          account, or the operator's key with --data);
                          the file is [[mod]] tables of map_id, authors,
                          release, released, source_url, notes; packages
                          already credited so are left alone
  analyze FILE --base MAP [--disc ISO] [--mappack ISO] [--format F]
                [--map-id ID] [--out REPORT.toml]
  analyze --map-id ID [--server URL] [--disc ISO] [--mappack ISO] [--out ..]
                          what a mod (a modded .map, or a .ppf/.ips/.bps;
                          or a published package's layer) contains over
                          your disc's map: tag classes, swaps, stats,
                          placements; --out writes the report for
                          details --contents
  details --file DETAILS.toml [--images DIR] [--contents DIR] [--dry-run]
                [--server URL] [--as GAMERTAG] [--data DIR]
                          set published packages' descriptions, screenshots
                          (DIR/<slug>/, uploaded by --as), contents reports
                          (DIR/<slug>.toml) and map packs, as an admin (or
                          the operator's key with --data); what already
                          says so is left alone

  Categories: gameplay, weapons, vehicles, visuals, maps, other.";

pub fn run(mut args: Args) -> Result<()> {
    match args.0.first().map(String::as_str) {
        Some("publish") => {
            args = args.shift();
            let server = args.take("--server")?;
            let as_tag = args.take("--as")?;
            let category = args.take("--category")?;
            let source_url = args.take("--source-url")?;
            let [dir] = args
                .positional_for(USAGE)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("publish takes one package directory\n\n{USAGE}"))?;
            let client = Client::connect(server)?;
            let answer = publish(
                &client,
                &accounts_path()?,
                as_tag.as_deref(),
                Path::new(&dir),
                category,
                source_url,
            )?;
            print_published(&answer);
            Ok(())
        }
        Some("import") => {
            args = args.shift();
            let server = args.take("--server")?;
            let as_tag = args.take("--as")?;
            let base = args.take("--base")?;
            let disc = args.take("--disc")?;
            let mappack = args.take("--mappack")?;
            let out = args.take("--out")?;
            let name = args.take("--name")?;
            let info = args.take("--info")?;
            let category = args.take("--category")?;
            let no_publish = args.flag("--no-publish");
            let [file] = args
                .positional_for(USAGE)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("import takes one mod file\n\n{USAGE}"))?;
            let disc = match disc {
                Some(d) => std::path::PathBuf::from(d),
                None => crate::default_disc()?,
            };
            // As `h2 play` finds it: mappack.iso beside the disc.
            let mappack = mappack
                .map(std::path::PathBuf::from)
                .or_else(|| crate::mappack_cmd::discover(&disc));
            let made = import(&ImportOptions {
                input: file.into(),
                disc,
                mappack,
                base,
                out: out.map(Into::into),
                info: info.map(Into::into),
                name,
                category,
            })?;
            println!(
                "{}: {} on {}{}, a {} layer for a {} mod",
                made.dir.display(),
                made.manifest.package.name,
                made.manifest.base.map,
                match made.base_from {
                    crate::live_mod_import::BaseFrom::Disc => "",
                    crate::live_mod_import::BaseFrom::MapPack => " (map pack)",
                },
                fmt_size(made.layer_size),
                fmt_size(made.input_size)
            );
            if no_publish {
                return Ok(());
            }
            let client = Client::connect(server)?;
            let answer = publish(
                &client,
                &accounts_path()?,
                as_tag.as_deref(),
                &made.dir,
                None,
                None,
            )?;
            print_published(&answer);
            Ok(())
        }
        Some("list") => {
            args = args.shift();
            let server = args.take("--server")?;
            let base = args.take("--base")?;
            let author = args.take("--author")?;
            let as_json = args.flag("--json");
            if !args.positional_for(USAGE)?.is_empty() {
                bail!("list takes no positional arguments\n\n{USAGE}");
            }
            let client = Client::connect(server)?;
            let maps = list(&client, base, author)?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&maps)?);
            } else if maps.is_empty() {
                println!("no map packages on {}", client.base);
            } else {
                for m in &maps {
                    println!(
                        "{}  {}  by {} on {}  {}",
                        m.map_id,
                        m.name,
                        m.author.gamertag,
                        m.base.map,
                        fmt_size(m.size)
                    );
                }
            }
            Ok(())
        }
        Some("credit") => {
            args = args.shift();
            let server = args.take("--server")?;
            let as_tag = args.take("--as")?;
            let data = args.take("--data")?;
            let file = args.take("--file")?;
            let dry_run = args.flag("--dry-run");
            let clear = args.flag("--clear");
            let mut authors = Vec::new();
            while let Some(a) = args.take("--author")? {
                authors.push(a);
            }
            let flags = Credit {
                authors,
                release: args.take("--release")?,
                released: args.take("--released")?,
                source_url: args.take("--source-url")?,
                notes: args.take("--notes")?,
            }
            .tidy();
            let positional = args.positional_for(USAGE)?;
            let wanted: Vec<CreditEntry> = match (file, positional.as_slice()) {
                (Some(path), []) => {
                    if !flags.is_empty() || clear {
                        bail!("--file takes its credits from the file, not flags");
                    }
                    read_credits(Path::new(&path))?
                }
                (None, [id]) => {
                    if flags.is_empty() != clear {
                        bail!("give the credit's flags, or --clear to drop an admin's credit");
                    }
                    vec![CreditEntry {
                        map_id: id
                            .parse()
                            .map_err(|_| anyhow::anyhow!("{id:?} is not a MapId"))?,
                        name: None,
                        credit: flags,
                    }]
                }
                _ => bail!("credit takes one MAP_ID or --file\n\n{USAGE}"),
            };
            let client = Client::connect(server)?;
            let signer = match data {
                Some(d) => Signer::Operator(crate::live_cmd::operator(Some(d), &client)?),
                None => {
                    Signer::Account(pick_account(&client, &accounts_path()?, as_tag.as_deref())?)
                }
            };
            let done = apply_credits(&client, &signer, &wanted, dry_run)?;
            for line in done {
                println!("{line}");
            }
            Ok(())
        }
        Some("analyze") => {
            args = args.shift();
            let server = args.take("--server")?;
            let base = args.take("--base")?;
            let disc = args.take("--disc")?;
            let mappack = args.take("--mappack")?;
            let format = args.take("--format")?;
            let map_id = args.take("--map-id")?;
            let out = args.take("--out")?;
            let file = match args.positional_for(USAGE)?.as_slice() {
                [] => None,
                [f] => Some(std::path::PathBuf::from(f)),
                _ => bail!("analyze takes one mod file\n\n{USAGE}"),
            };
            let client = match &file {
                Some(_) => None,
                None => Some(Client::connect(server)?),
            };
            let disc = match disc {
                Some(d) => std::path::PathBuf::from(d),
                None => crate::default_disc()?,
            };
            let mappack = mappack
                .map(std::path::PathBuf::from)
                .or_else(|| crate::mappack_cmd::discover(&disc));
            let a = crate::live_maps_details::analyze(
                &crate::live_maps_details::AnalyzeOptions {
                    input: file,
                    disc,
                    mappack,
                    base,
                    format,
                    map_id,
                },
                client.as_ref(),
            )?;
            for line in &a.contents.lines {
                println!("{line}");
            }
            if a.contents.lines.is_empty() {
                println!("(no changes found)");
            }
            if let Some(out) = out {
                std::fs::write(&out, toml::to_string(&a)?)
                    .with_context(|| format!("writing {out}"))?;
                println!("wrote {out}");
            }
            Ok(())
        }
        Some("details") => {
            args = args.shift();
            let server = args.take("--server")?;
            let as_tag = args.take("--as")?;
            let data = args.take("--data")?;
            let file = args
                .take("--file")?
                .context("details takes --file DETAILS.toml")?;
            let images = args.take("--images")?.map(std::path::PathBuf::from);
            let contents = args.take("--contents")?.map(std::path::PathBuf::from);
            let dry_run = args.flag("--dry-run");
            if !args.positional_for(USAGE)?.is_empty() {
                bail!("details takes no positional arguments\n\n{USAGE}");
            }
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {file}"))?;
            let parsed =
                crate::live_maps_details::DetailsFile::parse(&text).context(file.clone())?;
            let (packs, mods) =
                crate::live_maps_details::wanted(&parsed, images.as_deref(), contents.as_deref())?;
            let client = Client::connect(server)?;
            let account = match (&data, &as_tag) {
                (Some(_), None) => None,
                _ => Some(Signer::Account(pick_account(
                    &client,
                    &accounts_path()?,
                    as_tag.as_deref(),
                )?)),
            };
            let admin = match data {
                Some(d) => Signer::Operator(crate::live_cmd::operator(Some(d), &client)?),
                None => {
                    Signer::Account(pick_account(&client, &accounts_path()?, as_tag.as_deref())?)
                }
            };
            let done = crate::live_maps_details::apply_details(
                &client,
                &admin,
                account.as_ref(),
                &packs,
                &mods,
                dry_run,
            )?;
            for line in done {
                println!("{line}");
            }
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

/// One `[[mod]]` of a credits file: the package and who made it. Other
/// keys (`name`, `base`, `confidence`, `evidence`) are for the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreditEntry {
    pub map_id: MapId,
    /// Checked against the server's name, when given.
    pub name: Option<String>,
    pub credit: Credit,
}

#[derive(serde::Deserialize)]
struct CreditsFile {
    #[serde(default, rename = "mod")]
    mods: Vec<CreditRow>,
}

#[derive(serde::Deserialize)]
struct CreditRow {
    map_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    released: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

/// The credits in `path`, each checked as the server checks it.
pub(crate) fn read_credits(path: &Path) -> Result<Vec<CreditEntry>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_credits(&text).with_context(|| path.display().to_string())
}

pub(crate) fn parse_credits(text: &str) -> Result<Vec<CreditEntry>> {
    let file: CreditsFile = toml::from_str(text)?;
    let mut seen = std::collections::BTreeSet::new();
    file.mods
        .into_iter()
        .map(|r| {
            let map_id: MapId = r
                .map_id
                .parse()
                .map_err(|_| anyhow::anyhow!("{:?} is not a MapId", r.map_id))?;
            if !seen.insert(map_id) {
                bail!("{map_id} is credited twice");
            }
            let credit = Credit {
                authors: r.authors,
                release: r.release,
                released: r.released,
                source_url: r.source_url,
                notes: r.notes,
            }
            .tidy();
            credit
                .validate()
                .map_err(|e| anyhow::anyhow!("{map_id}: {e}"))?;
            Ok(CreditEntry {
                map_id,
                name: r.name,
                credit,
            })
        })
        .collect()
}

/// Who signs the credit's token: an admin's account, or the operator.
pub(crate) enum Signer {
    Account(crate::live_accounts::LocalAccount),
    Operator(live_server::ServerIdentity),
}

impl Signer {
    pub(crate) fn token(&self, client: &Client, method: &str) -> Result<String> {
        Ok(match self {
            Signer::Account(me) => {
                let key = me.key_pair()?;
                let author = me
                    .id
                    .clone()
                    .unwrap_or_else(|| live_accounts::author_id(me.xuid, &client.discovery.id));
                issue_token(&key, &author, &client.discovery.id, method, now())
            }
            Signer::Operator(op) => issue_token(&op.key, &op.author(), &op.id, method, now()),
        })
    }
}

/// Puts each credit that differs from the server's (all of them when an
/// entry clears), or only says what it would with `dry_run`. One line per
/// package. A package the server does not list, or lists under another
/// name than the entry's, is an error before anything is sent.
pub(crate) fn apply_credits(
    client: &Client,
    signer: &Signer,
    wanted: &[CreditEntry],
    dry_run: bool,
) -> Result<Vec<String>> {
    let listed: std::collections::HashMap<MapId, MapSummary> = list(client, None, None)?
        .into_iter()
        .map(|m| (m.map_id, m))
        .collect();
    for w in wanted {
        let m = listed
            .get(&w.map_id)
            .with_context(|| format!("{}: the server lists no such package", w.map_id))?;
        if let Some(name) = &w.name
            && name.trim() != m.name
        {
            bail!(
                "{}: the server names it {:?}, the credit {:?}",
                w.map_id,
                m.name,
                name
            );
        }
    }
    let mut out = Vec::new();
    for w in wanted {
        let m = &listed[&w.map_id];
        let now_credit = m.credits.clone().unwrap_or_default();
        let say = match w.credit.names() {
            Some(n) => n,
            None if w.credit.is_empty() => "(cleared)".into(),
            None => "an unknown author".into(),
        };
        if !w.credit.is_empty() && now_credit == w.credit {
            out.push(format!("{}  {}: already {say}", w.map_id, m.name));
            continue;
        }
        if dry_run {
            out.push(format!(
                "{}  {}: would credit {say} (now {})",
                w.map_id,
                m.name,
                m.credit.as_deref().unwrap_or("none")
            ));
            continue;
        }
        let req = client
            .agent
            .put(&client.url(&format!("/maps/{}/credit", w.map_id)))
            .set("content-type", "application/json");
        let req = client.authorized(req, signer.token(client, lxm::CONTENT_MODERATE)?)?;
        let body = serde_json::to_vec(&w.credit)?;
        call(req, Some(&body)).with_context(|| format!("crediting {} ({})", m.name, w.map_id))?;
        out.push(format!("{}  {}: credited {say}", w.map_id, m.name));
    }
    Ok(out)
}

fn print_published(answer: &MapAnswer) {
    let m = &answer.map.summary;
    println!(
        "published {} ({}) on {}: {}",
        m.name,
        fmt_size(m.size),
        m.base.map,
        m.map_id
    );
}

fn fmt_size(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MiB", n as f64 / f64::from(1 << 20)),
        n if n >= 1 << 10 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{n} bytes"),
    }
}

/// Every map package on the server (all pages).
pub(crate) fn list(
    client: &Client,
    base: Option<String>,
    author: Option<String>,
) -> Result<Vec<MapSummary>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut q = vec![("limit", "1000".to_string())];
        q.extend(base.clone().map(|b| ("base", b)));
        q.extend(author.clone().map(|a| ("author", a)));
        q.extend(cursor.clone().map(|c| ("cursor", c)));
        let page: Page<MapSummary> = client.get("/maps", &q)?;
        out.extend(page.items);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
}

/// Uploads `dir`'s layers and image, then publishes its manifest as the
/// local account `as_tag` (by default the only one on this server), with
/// the repository's extras from `dir/mod.toml` unless `category` or
/// `source_url` are given.
pub(crate) fn publish(
    client: &Client,
    accounts: &Path,
    as_tag: Option<&str>,
    dir: &Path,
    category: Option<String>,
    source_url: Option<String>,
) -> Result<MapAnswer> {
    let me = pick_account(client, accounts, as_tag)?;
    publish_member(client, &me, dir, category, source_url, None)
}

pub(crate) fn publish_member(
    client: &Client,
    me: &live_accounts::LocalAccount,
    dir: &Path,
    category: Option<String>,
    source_url: Option<String>,
    pack: Option<&str>,
) -> Result<MapAnswer> {
    let path = dir.join(MANIFEST_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest = Manifest::from_toml(&text).with_context(|| path.display().to_string())?;
    manifest
        .validate_in(dir)
        .with_context(|| path.display().to_string())?;
    let extras = extras_in(dir)?;
    let mut doc = MapDocument::new(manifest.clone(), None);
    doc.staged_for = pack.map(str::to_owned);
    doc.category = category.or(extras.category);
    doc.source_url = source_url.or(extras.source_url);
    doc.credit = extras.credit;
    // The Mods page's checklist, game types and version (an older server
    // refuses a document with them: publish those once it is updated).
    crate::live_mod_import::PageExtras::read(dir)?.apply(&mut doc);
    doc.validate_extras().map_err(|e| anyhow::anyhow!(e))?;
    let key = me.key_pair()?;
    let author = me
        .id
        .clone()
        .unwrap_or_else(|| live_accounts::author_id(me.xuid, &client.discovery.id));
    let token = |method: &str| issue_token(&key, &author, &client.discovery.id, method, now());

    let mut files: Vec<(&str, Option<Sha256>)> = manifest
        .patches
        .iter()
        .chain(&manifest.packs)
        .map(|l| (l.file.as_str(), Some(l.sha256)))
        .collect();
    if let Some(image) = &manifest.package.image {
        files.push((image, None));
    }
    let mut image_hash = None;
    for (file, want) in files {
        let p = dir.join(relative(file)?);
        let bytes = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
        let sha = Sha256::of(&bytes);
        match want {
            Some(w) if w != sha => bail!("{file}: its SHA-256 is {sha}, the manifest says {w}"),
            Some(_) => {}
            None => image_hash = Some(sha),
        }
        let req = client
            .agent
            .put(&client.url(&format!("/content/{sha}")))
            .set("content-type", "application/octet-stream");
        write_package_request(pack.is_some(), || {
            let req = client.authorized(req.clone(), token(lxm::CONTENT_WRITE))?;
            call(req, Some(&bytes))
        })
        .with_context(|| format!("uploading {file}"))?;
    }

    let id = MapId::of(&manifest);
    doc.image = image_hash;
    let jws = EventBody::at_unix(
        &key,
        &author,
        &package_stream(&id),
        MAP_PUBLISHED,
        serde_json::to_value(&doc).expect("serializes"),
        now(),
    )
    .sign(&key);
    let req = client
        .agent
        .put(&client.url("/maps"))
        .set("content-type", "application/jose");
    let text = write_package_request(pack.is_some(), || {
        let req = client.authorized(req.clone(), token(lxm::CONTENT_WRITE))?;
        call(req, Some(jws.as_bytes()))
    })
    .context("publishing the manifest")?;
    serde_json::from_str(&text).context("the server's answer to the publish")
}

// A pack can use the server's account burst allowance in a single upload.
// Retry only the idempotent write that was throttled, with a fresh token.
pub(crate) fn write_package_request<T>(
    bulk: bool,
    mut write: impl FnMut() -> Result<T>,
) -> Result<T> {
    for _ in 0..6 {
        match write() {
            Err(e) if bulk && crate::live_cmd::status_of(&e) == Some(429) => {
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
            result => return result,
        }
    }
    write()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_accounts::LocalAccount;
    use content::manifest::Layer;

    #[test]
    fn publish_uploads_layers_and_image_then_lists() {
        let dir = std::env::temp_dir().join(format!(
            "h2-live-maps-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir.join("server"),
            Default::default(),
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        let (created, key) = client.create_account("Modder", None).unwrap();
        let accounts = dir.join("accounts.json");
        let mut list_of = Vec::new();
        live_accounts::upsert(
            &mut list_of,
            LocalAccount::new(
                created.xuid,
                created.gamertag.as_str(),
                &client.base,
                &client.discovery,
                &key,
            ),
        );
        live_accounts::save(&accounts, &list_of).unwrap();

        // An invented package: one patch and a preview.
        let pkg = dir.join("snipers");
        std::fs::create_dir_all(&pkg).unwrap();
        let patch = b"PATCH invented for the cli".to_vec();
        std::fs::write(pkg.join("snipers.ips"), &patch).unwrap();
        std::fs::write(pkg.join("preview.png"), b"\x89PNG\r\n\x1a\ninvented").unwrap();
        let mut m = Manifest::retail("lockout", Sha256([0x21; 32]));
        m.package.name = "Lockout Snipers".into();
        m.package.image = Some("preview.png".into());
        m.patches = vec![Layer {
            file: "snipers.ips".into(),
            sha256: Sha256::of(&patch),
        }];
        std::fs::write(pkg.join(MANIFEST_FILE), m.to_toml()).unwrap();

        let answer = publish(&client, &accounts, None, &pkg, None, None).unwrap();
        assert_eq!(answer.map.summary.map_id, MapId::of(&m));
        assert_eq!(answer.map.summary.author.gamertag, "Modder");
        assert!(answer.map.summary.image.is_some());
        assert!(answer.missing.is_empty());
        let all = list(&client, None, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Lockout Snipers");
        assert!(
            list(&client, Some("midship".into()), None)
                .unwrap()
                .is_empty()
        );

        // A layer whose bytes are not the manifest's is never uploaded.
        std::fs::write(pkg.join("snipers.ips"), b"PATCH changed").unwrap();
        let err = publish(&client, &accounts, None, &pkg, None, None).unwrap_err();
        assert!(format!("{err:#}").contains("SHA-256"), "{err:#}");
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credits_files_read_and_are_checked() {
        let id = "ab".repeat(32);
        let ok = parse_credits(&format!(
            r#"
[[mod]]
map_id = "{id}"
name = "Beasthog"
base = "zanzibar"
authors = [" Group Bocked ", ""]
released = "2005"
source_url = "https://example.invalid/b"
confidence = "high"
evidence = "readme"
"#
        ))
        .unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].map_id.to_string(), id);
        assert_eq!(ok[0].name.as_deref(), Some("Beasthog"));
        assert_eq!(ok[0].credit.authors, ["Group Bocked"]);
        assert_eq!(ok[0].credit.released.as_deref(), Some("2005"));
        assert!(parse_credits("").unwrap().is_empty());
        let twice = format!("[[mod]]\nmap_id = \"{id}\"\n[[mod]]\nmap_id = \"{id}\"\n");
        assert!(format!("{:#}", parse_credits(&twice).unwrap_err()).contains("twice"));
        let bad_date = format!("[[mod]]\nmap_id = \"{id}\"\nreleased = \"soon\"\n");
        assert!(parse_credits(&bad_date).is_err());
        assert!(parse_credits("[[mod]]\nmap_id = \"nope\"\n").is_err());
    }

    #[test]
    fn the_published_mods_credits_file_reads() {
        let all = parse_credits(include_str!("../../../../docs/game/mod-credits.toml")).unwrap();
        assert!(all.len() >= 47, "{}", all.len());
        assert!(
            all.iter().all(|e| e.name.is_some()),
            "every entry names its package"
        );
        // Nobody is credited as the uploader.
        assert!(all.iter().all(|e| {
            !e.credit
                .authors
                .iter()
                .any(|a| a.eq_ignore_ascii_case("afr"))
        }));
    }

    #[test]
    fn the_operator_credits_published_packages_from_a_file() {
        let dir = std::env::temp_dir().join(format!(
            "h2-live-credit-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir.join("server"),
            Default::default(),
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        let (created, key) = client.create_account("Importer", None).unwrap();
        let accounts = dir.join("accounts.json");
        let mut list_of = Vec::new();
        live_accounts::upsert(
            &mut list_of,
            LocalAccount::new(
                created.xuid,
                created.gamertag.as_str(),
                &client.base,
                &client.discovery,
                &key,
            ),
        );
        live_accounts::save(&accounts, &list_of).unwrap();
        let pkg = dir.join("hog");
        std::fs::create_dir_all(&pkg).unwrap();
        let patch = b"PATCH invented hog".to_vec();
        std::fs::write(pkg.join("hog.ips"), &patch).unwrap();
        let mut m = Manifest::retail("zanzibar", Sha256([0x22; 32]));
        m.package.name = "Beasthog".into();
        m.package.author = Some("A Guess".into());
        m.patches = vec![Layer {
            file: "hog.ips".into(),
            sha256: Sha256::of(&patch),
        }];
        std::fs::write(pkg.join(MANIFEST_FILE), m.to_toml()).unwrap();
        let id = publish(&client, &accounts, None, &pkg, None, None)
            .unwrap()
            .map
            .summary
            .map_id;

        let wanted = parse_credits(&format!(
            "[[mod]]\nmap_id = \"{id}\"\nname = \"Beasthog\"\nauthors = [\"Group Bocked\"]\nreleased = \"2005\"\n"
        ))
        .unwrap();
        let op = Signer::Operator(
            crate::live_cmd::operator(Some(dir.join("server").display().to_string()), &client)
                .unwrap(),
        );
        let said = apply_credits(&client, &op, &wanted, true).unwrap();
        assert!(
            said[0].contains("would credit Group Bocked (now A Guess)"),
            "{said:?}"
        );
        assert_eq!(
            list(&client, None, None).unwrap()[0].credit.as_deref(),
            Some("A Guess")
        );
        let said = apply_credits(&client, &op, &wanted, false).unwrap();
        assert!(said[0].ends_with("credited Group Bocked"), "{said:?}");
        let listed = &list(&client, None, None).unwrap()[0];
        assert_eq!(listed.credit.as_deref(), Some("Group Bocked"));
        assert_eq!(listed.author.gamertag, "Importer");
        let said = apply_credits(&client, &op, &wanted, false).unwrap();
        assert!(said[0].ends_with("already Group Bocked"), "{said:?}");

        // A plain account is no admin; a wrong name or id sends nothing.
        let me = Signer::Account(pick_account(&client, &accounts, None).unwrap());
        let mut other = wanted.clone();
        other[0].credit.authors = vec!["Someone".into()];
        assert!(apply_credits(&client, &me, &other, false).is_err());
        other[0].name = Some("Not Beasthog".into());
        assert!(apply_credits(&client, &op, &other, false).is_err());
        other[0].name = None;
        other[0].map_id = MapId([7; 32]);
        assert!(apply_credits(&client, &op, &other, false).is_err());
        assert_eq!(
            list(&client, None, None).unwrap()[0].credit.as_deref(),
            Some("Group Bocked")
        );
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(fmt_size(12), "12 bytes");
        assert_eq!(fmt_size(2048), "2.0 KiB");
        assert_eq!(fmt_size(3 << 20), "3.0 MiB");
    }
}
