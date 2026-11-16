//! `h2 live maps publish|list`: share a map package over Live
//! (docs/game/maps.md §5, docs/live/live-api.md "Custom maps").
//!
//! `publish DIR` reads the package's `manifest.toml` (checked as the
//! runtime checks it), uploads every layer and the preview image to the
//! content store by SHA-256, then signs the manifest as a `MapPublished`
//! event with the local account's key and puts it. `list` prints what the
//! server has.
use std::path::Path;

use anyhow::{Context, Result, bail};
use content::manifest::{MANIFEST_FILE, relative};
use content::{Manifest, MapId, Sha256};
use live_proto::maps::{MAP_PUBLISHED, MapAnswer, MapDocument, MapSummary, package_stream};
use live_proto::sign::{EventBody, issue_token};
use live_proto::{Page, lxm};

use crate::live_accounts;
use crate::live_cmd::{Args, Client, accounts_path, call, now, pick_account};

pub const USAGE: &str = "usage: h2 live maps <command>

  publish DIR [--server URL] [--as GAMERTAG]
                          upload a map package (DIR/manifest.toml, its
                          layers and image) and publish it as yours
  list [--server URL] [--base MAP] [--author GAMERTAG] [--json]
                          the packages the server has";

pub fn run(mut args: Args) -> Result<()> {
    match args.0.first().map(String::as_str) {
        Some("publish") => {
            args = args.shift();
            let server = args.take("--server")?;
            let as_tag = args.take("--as")?;
            let [dir] = args
                .positional_for(USAGE)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("publish takes one package directory\n\n{USAGE}"))?;
            let client = Client::connect(server)?;
            let answer = publish(&client, &accounts_path()?, as_tag.as_deref(), Path::new(&dir))?;
            let m = &answer.map.summary;
            println!(
                "published {} ({}) on {}: {}",
                m.name,
                fmt_size(m.size),
                m.base.map,
                m.map_id
            );
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
        _ => bail!("{USAGE}"),
    }
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
/// local account `as_tag` (by default the only one on this server).
pub(crate) fn publish(
    client: &Client,
    accounts: &Path,
    as_tag: Option<&str>,
    dir: &Path,
) -> Result<MapAnswer> {
    let path = dir.join(MANIFEST_FILE);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest = Manifest::from_toml(&text).with_context(|| path.display().to_string())?;
    manifest
        .validate_in(dir)
        .with_context(|| path.display().to_string())?;
    let me = pick_account(client, accounts, as_tag)?;
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
        let req = client.authorized(req, token(lxm::CONTENT_WRITE))?;
        call(req, Some(&bytes)).with_context(|| format!("uploading {file}"))?;
    }

    let id = MapId::of(&manifest);
    let doc = MapDocument {
        manifest,
        image: image_hash,
    };
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
    let req = client.authorized(req, token(lxm::CONTENT_WRITE))?;
    let text = call(req, Some(jws.as_bytes())).context("publishing the manifest")?;
    serde_json::from_str(&text).context("the server's answer to the publish")
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

        let answer = publish(&client, &accounts, None, &pkg).unwrap();
        assert_eq!(answer.map.summary.map_id, MapId::of(&m));
        assert_eq!(answer.map.summary.author.gamertag, "Modder");
        assert!(answer.map.summary.image.is_some());
        assert!(answer.missing.is_empty());
        let all = list(&client, None, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Lockout Snipers");
        assert!(list(&client, Some("midship".into()), None).unwrap().is_empty());

        // A layer whose bytes are not the manifest's is never uploaded.
        std::fs::write(pkg.join("snipers.ips"), b"PATCH changed").unwrap();
        let err = publish(&client, &accounts, None, &pkg).unwrap_err();
        assert!(format!("{err:#}").contains("SHA-256"), "{err:#}");
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
