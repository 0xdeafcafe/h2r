//! `h2 live admin grant|revoke`, `h2 live hopper import|defaults` and `h2
//! live account export` (docs/live/live-admin.md). Dispatched from `live_cmd.rs`.
//!
//! The admin commands sign with the server key in `--data`, like `motd set`
//! and `invite create`, so they run on the server's machine. `account
//! export` runs anywhere the accounts file is: it turns a CLI-held account
//! into exactly the entry the web app's Admin area imports, after checking
//! with the server that the key is still current — the mirror of what
//! `account import` checks in the other direction.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use live_proto::hopper::{HALO_2_HOPPER_FILE, HOPPERS, HopperFile, INFLATED_SIZE};
use live_proto::sign::{KeyPair, issue_token};
use live_proto::{Gamertag, HALO_2_TITLE_ID, PublishedFile, TitleId, lxm};

use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{self, Args, Client};

const USAGE: &str = "usage: h2 live admin grant|revoke <gamertag> [--server URL] [--data DIR]
       h2 live invite grant <gamertag> [--open-cap N] [--server URL] [--data DIR]
       h2 live invite ungrant <gamertag> [--server URL] [--data DIR]
       h2 live hopper import <file> [--title ID] [--server URL] [--data DIR]
       h2 live hopper defaults --out FILE [--raw] [--force]
       h2 live account export <gamertag> [--file PATH] [--server URL]";

/// A gamertag for a URL path segment: percent-encoded, since gamertags may
/// hold spaces.
fn path_segment(tag: &Gamertag) -> String {
    let mut out = String::with_capacity(tag.as_str().len());
    for b in tag.as_str().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn one_tag(args: Vec<String>) -> Result<Gamertag> {
    let [tag] = args.try_into().map_err(|_| {
        anyhow::anyhow!("takes one gamertag (quote it if it has spaces)\n\n{USAGE}")
    })?;
    Gamertag::new(&tag).with_context(|| format!("{tag:?} is not a valid gamertag"))
}

/// `h2 live admin grant <gamertag>`: the operator makes an account an admin
/// (a server-signed `AdminGranted` event on its account stream).
pub fn admin_grant(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let data = args.take("--data")?;
    let tag = one_tag(args.positional_for(USAGE)?)?;
    let client = Client::connect(server)?;
    let me = live_cmd::operator(data, &client)?;
    let token = issue_token(
        &me.key,
        &me.author(),
        &me.id,
        live_server::admin::LXM_ADMIN_WRITE,
        live_cmd::now(),
    );
    let req = client
        .agent
        .post(&client.url(&format!("/admins/{}", path_segment(&tag))));
    let req = client.authorized(req, token)?;
    let reply: serde_json::Value = serde_json::from_str(&live_cmd::call(req, Some(b""))?)?;
    println!(
        "granted {tag} (position {})",
        reply["position"].as_u64().unwrap_or(0)
    );
    Ok(())
}

/// `h2 live admin revoke <gamertag>`.
pub fn admin_revoke(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let data = args.take("--data")?;
    let tag = one_tag(args.positional_for(USAGE)?)?;
    let client = Client::connect(server)?;
    let me = live_cmd::operator(data, &client)?;
    let token = issue_token(
        &me.key,
        &me.author(),
        &me.id,
        live_server::admin::LXM_ADMIN_WRITE,
        live_cmd::now(),
    );
    let req = client
        .agent
        .delete(&client.url(&format!("/admins/{}", path_segment(&tag))));
    let req = client.authorized(req, token)?;
    let reply: serde_json::Value = serde_json::from_str(&live_cmd::call(req, None)?)?;
    println!(
        "revoked {tag} (position {})",
        reply["position"].as_u64().unwrap_or(0)
    );
    Ok(())
}

/// The operator enables or ends an account's explicit invite permission.
pub(crate) fn invite_grant(mut args: Args, enabled: bool) -> Result<()> {
    let server = args.take("--server")?;
    let data = args.take("--data")?;
    let cap = if enabled {
        args.take("--open-cap")?
            .map(|cap| cap.parse::<std::num::NonZeroU32>())
            .transpose()
            .context("--open-cap must be a positive whole number")?
    } else {
        None
    };
    let tag = one_tag(args.positional_for(USAGE)?)?;
    let client = Client::connect(server)?;
    let me = live_cmd::operator(data, &client)?;
    let token = issue_token(
        &me.key,
        &me.author(),
        &me.id,
        lxm::INVITE_GRANTS_WRITE,
        live_cmd::now(),
    );
    let url = client.url(&format!("/accounts/{}/invite-grant", path_segment(&tag)));
    let (req, body) = if enabled {
        let body = serde_json::to_vec(&live_proto::InviteGrant { open_cap: cap })?;
        (
            client.agent.post(&url).set("Content-Type", "application/json"),
            Some(body),
        )
    } else {
        (client.agent.delete(&url), None)
    };
    let reply: serde_json::Value =
        serde_json::from_str(&live_cmd::call(client.authorized(req, token)?, body.as_deref())?)?;
    let action = if enabled { "enabled" } else { "revoked" };
    println!(
        "invite permission {action} for {tag} (position {})",
        reply["position"].as_u64().unwrap_or(0)
    );
    Ok(())
}

/// A hopper file as the owner's console leaves it (the raw 0x162cc0-byte
/// image, `n:\multiplayer_hopper.dat`) or as it is downloaded (the envelope
/// the server publishes).
pub fn read_hopper(bytes: &[u8]) -> Result<HopperFile> {
    if bytes.len() == INFLATED_SIZE {
        HopperFile::from_image(bytes).map_err(|e| anyhow::anyhow!("{e}"))
    } else {
        HopperFile::parse_legacy(bytes).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// `h2 live hopper import <file>`: publishes a base hopper image as the
/// title file, so the admin API and the web builder have something to edit.
/// Never puts the bytes anywhere but the server.
pub fn hopper_import(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let data = args.take("--data")?;
    let title = args
        .take("--title")?
        .unwrap_or_else(|| HALO_2_TITLE_ID.to_string());
    let [path] = args
        .positional_for(USAGE)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("hopper import takes one file\n\n{USAGE}"))?;
    let title = TitleId::parse(&title)
        .with_context(|| format!("{title:?} is not a title id (8 hex digits)"))?;
    let bytes = fs::read(&path).with_context(|| format!("reading {path}"))?;
    let file = read_hopper(&bytes)?;
    for i in 0..HOPPERS {
        let h = file.hopper(i)?;
        if h.record_count > 0 {
            println!(
                "  {:>2}  type {}  {:3} entries  {}",
                i,
                h.hopper_type,
                h.record_count,
                file.text(i)?.title
            );
        }
    }
    let built = file.build_checked().map_err(|e| anyhow::anyhow!("{e}"))?;
    publish_title_file(&built, title, HALO_2_HOPPER_FILE, server, data)
}

/// `h2 live hopper defaults --out FILE`: writes the built-in playlists
/// (`HopperFile::default_set`, what a server serves with no file
/// published) as the download envelope, or with `--raw` as the 0x162cc0-byte
/// image a console caches. Offline: nothing is sent anywhere.
pub fn hopper_defaults(mut args: Args) -> Result<()> {
    let out = args
        .take("--out")?
        .ok_or_else(|| anyhow::anyhow!("hopper defaults needs --out FILE\n\n{USAGE}"))?;
    let raw = args.flag("--raw");
    let force = args.flag("--force");
    let extra = args.positional_for(USAGE)?;
    if !extra.is_empty() {
        bail!("hopper defaults takes no other arguments\n\n{USAGE}");
    }
    let file = HopperFile::default_set();
    let size = write_defaults(&file, Path::new(&out), raw, force)?;
    for i in 0..HOPPERS {
        let d = file.descriptor(i)?;
        if d.hopper_type != 0 {
            println!(
                "  {:>2}  type {}  {:3} entries  {}",
                i,
                d.hopper_type,
                file.entry_count(i)?,
                d.titles[0]
            );
        }
    }
    println!("wrote {out} ({size} bytes)");
    Ok(())
}

/// Writes `file` to `dest` (the envelope, or the raw image), never over a
/// disc image and only over an existing file with `force`. Returns the size.
fn write_defaults(file: &HopperFile, dest: &Path, raw: bool, force: bool) -> Result<usize> {
    let bytes = if raw {
        file.image().to_vec()
    } else {
        file.build_checked().map_err(|e| anyhow::anyhow!("{e}"))?
    };
    crate::safe_out::guard_target(dest, None)?;
    let name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} is not a file path", dest.display()))?
        .to_string();
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    crate::safe_out::OutDir::new(parent, force)?
        .write(&[name], &bytes)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("writing {}", dest.display()))?;
    Ok(bytes.len())
}

/// The shared tail of the operator file-publishing commands (`motd set`,
/// `hopper import`): sign with the server key, PUT, report.
fn publish_title_file(
    bytes: &[u8],
    title: TitleId,
    name: &str,
    server: Option<String>,
    data: Option<String>,
) -> Result<()> {
    if bytes.len() > live_server::MAX_TITLE_FILE {
        bail!(
            "the file is {} bytes; title files are at most {} MiB",
            bytes.len(),
            live_server::MAX_TITLE_FILE >> 20
        );
    }
    let client = Client::connect(server)?;
    let me = live_cmd::operator(data, &client)?;
    let token = issue_token(
        &me.key,
        &me.author(),
        &me.id,
        lxm::TITLES_WRITE,
        live_cmd::now(),
    );
    let req = client
        .agent
        .put(&client.url(&format!("/titles/{title}/files/{name}")));
    let req = client.authorized(req.set("content-type", "application/octet-stream"), token)?;
    let published: PublishedFile = serde_json::from_str(&live_cmd::call(req, Some(bytes))?)?;
    println!(
        "published {} ({} bytes, sha256 {})",
        published.name, published.size, published.sha256
    );
    Ok(())
}

/// The web app's account entry (`src/state/account.ts`, `LocalAccount`):
/// what its Admin-area import and `/signin` link (`h2 play --live-local`) accept,
/// and what sign-up's download produces.
pub(crate) fn export_entry(
    entry: &LocalAccount,
    base: &str,
    server_id: &str,
    key: &KeyPair,
) -> serde_json::Value {
    serde_json::json!({
        "xuid": entry.xuid,
        "gamertag": entry.gamertag,
        "id": live_accounts::author_id(entry.xuid, server_id),
        "server": base,
        "server_id": server_id,
        "kid": key.kid(),
        "key": key.secret_b64(),
        "x": key.public().jwk().x,
    })
}

/// `h2 live account export <gamertag> [--file PATH]`: the smallest safe way
/// to sign the browser in as an account whose key the CLI holds (the web
/// app can only sign up new ones). Checks the key is still current on the
/// server, then writes the browser's entry — to `--file` (mode 0600, it is
/// a private key) or to stdout for pasting. Import it on the stats site's
/// Admin page.
pub fn account_export(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let file = match args.take("--file")? {
        Some(f) => Some(f),
        None => args.take("--out")?,
    };
    let tag = one_tag(args.positional_for(USAGE)?)?;
    let path = live_cmd::accounts_path()?;
    let accounts = live_accounts::load(&path)?;
    let entry = accounts
        .iter()
        .find(|a| a.gamertag.eq_ignore_ascii_case(tag.as_str()))
        .with_context(|| {
            format!(
                "no account {tag} in {}; create or import it first",
                path.display()
            )
        })?;
    let client = Client::at(server.as_deref().unwrap_or(&entry.server))?;
    let key = entry.key_pair()?;
    // The same check `account import` makes in the other direction: the key
    // this machine holds is the account's current key on that server.
    let keys = client.keys(entry.xuid)?;
    let current = keys
        .keys
        .iter()
        .any(|k| k.valid_to.is_none() && k.kid == key.kid() && k.jwk.x == key.public().jwk().x);
    if !current {
        bail!(
            "the server no longer lists {tag}'s key as current; it cannot be \
             signed in anywhere"
        );
    }
    let entry = export_entry(entry, &client.base, &client.discovery.id, &key);
    match file {
        Some(path) => {
            live_server::identity::write_private(
                std::path::Path::new(&path),
                format!("{}\n", serde_json::to_string_pretty(&entry)?).as_bytes(),
            )
            .with_context(|| format!("writing {path}"))?;
            println!("wrote {path} (mode 0600)");
            println!("import it in the web app: Admin > Import account");
        }
        None => {
            eprintln!("this prints {tag}'s private key; prefer --file, or clear your scrollback");
            println!("{}", serde_json::to_string_pretty(&entry)?);
            println!("import it in the web app: Admin > Import account");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::Xuid;
    use live_proto::sign::KeyPair;
    use serde_json::json;

    #[test]
    fn gamertags_are_encoded_for_paths() {
        assert_eq!(path_segment(&Gamertag::new("afr").unwrap()), "afr");
        assert_eq!(
            path_segment(&Gamertag::new("Sneaky Pete").unwrap()),
            "Sneaky%20Pete"
        );
    }

    #[test]
    fn hopper_files_are_sniffed() {
        let file = HopperFile::blank();
        // The download envelope...
        let read = read_hopper(&file.build()).unwrap();
        assert_eq!(read.image(), file.image());
        // ...and the raw image the console caches.
        let read = read_hopper(file.image()).unwrap();
        assert_eq!(read.image(), file.image());
        assert!(read_hopper(b"not a hopper file").is_err());
    }

    #[test]
    fn hopper_defaults_writes_the_default_set() {
        let dir = std::env::temp_dir().join(format!(
            "h2-hopper-defaults-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = HopperFile::default_set();
        let out = dir.join("multiplayer_hopper.dat");
        let size = write_defaults(&file, &out, false, false).unwrap();
        let written = fs::read(&out).unwrap();
        assert_eq!(written.len(), size);
        let parsed = HopperFile::parse(&written).unwrap();
        assert!(parsed.image() == file.image());
        let maps = live_proto::hopper::defaults::retail_map_ids();
        assert_eq!(parsed.problems(&maps), vec![]);
        // No overwrite without --force.
        assert!(write_defaults(&file, &out, false, false).is_err());
        // The raw image is what a console caches, and import reads it.
        let raw = dir.join("raw.dat");
        assert_eq!(
            write_defaults(&file, &raw, true, false).unwrap(),
            INFLATED_SIZE
        );
        assert!(read_hopper(&fs::read(&raw).unwrap()).unwrap().image() == file.image());
        assert_eq!(
            write_defaults(&file, &out, true, true).unwrap(),
            INFLATED_SIZE
        );
        // Never a disc image.
        assert!(write_defaults(&file, &dir.join("x.iso"), false, true).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn exported_accounts_are_the_browsers_shape() {
        let key = KeyPair::generate();
        let entry = LocalAccount::new(
            Xuid(0x0009_0000_0000_0042),
            "afr",
            "http://127.0.0.1:8074",
            &live_proto::Discovery {
                ingest_frame_versions: Vec::new(),
                id: "localhost".into(),
                api: "/v1".into(),
                keys: Vec::new(),
                software: "test".into(),
                federation: false,
                url: None,
            },
            &key,
        );
        let exported = export_entry(&entry, "http://127.0.0.1:8074", "localhost", &key);
        assert_eq!(
            exported,
            json!({
                "xuid": "0009000000000042",
                "gamertag": "afr",
                "id": "acct:0009000000000042@localhost",
                "server": "http://127.0.0.1:8074",
                "server_id": "localhost",
                "kid": key.kid(),
                "key": key.secret_b64(),
                "x": key.public().jwk().x,
            })
        );
        // The seed really is the key the entry names.
        assert_eq!(
            KeyPair::from_secret_b64(exported["key"].as_str().unwrap())
                .unwrap()
                .kid(),
            key.kid()
        );
    }
}
