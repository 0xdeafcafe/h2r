//! `h2 live ...`: run the Xbox Live replacement service and use it
//! (docs/live.md §4.2, docs/live-security.md).
//!
//! Host data lives beside the other h2recomp data
//! (`~/Library/Application Support/h2recomp/live`):
//!
//! - `accounts.json` (0600): the accounts made here, each with its private
//!   key. The game-side HLE reads `xuid` and `gamertag` from it.
//! - `server/` (0700): `h2 live serve`'s identity (key and id). The CLI
//!   signs operator requests (`motd set`, `invite create`) with it, so they
//!   work on the machine the server runs on.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use live_proto::motd::Motd;
use live_proto::sign::{AccountRequest, Invite, KeyPair, issue_token};
use live_proto::{
    AccountSummary, CreatedAccount, Discovery, ErrorBody, Gamertag, HALO_2_MOTD_FILE,
    HALO_2_TITLE_ID, Page, PublishedFile, bearer_allowed, lxm,
};
use live_server::ServerIdentity;
use live_server::identity::write_private;
use serde_json::Value;

const USAGE: &str = "usage: h2 live <command> [options]

  serve [--db URL] [--listen ADDR] [--data DIR] [--id HOST]
        [--private] [--invite-only]
        [--tls-cert PEM --tls-key PEM | --insecure-http]
                          run the service. --db defaults to $H2_LIVE_DB or
                          postgres://localhost/h2live, --listen to
                          127.0.0.1:8074; a non-loopback address needs TLS or
                          --insecure-http. --id (fixed on first start) names
                          the server in tokens and author ids
  account create <gamertag> [--server URL] [--invite CODE]
                          make a key, register it, keep both in accounts.json
  account list [--server URL]
  motd set <line>... [--header TEXT] [--server URL] [--data DIR]
  motd set --file PATH [--server URL] [--data DIR]
                          publish Halo 2's message_of_the_day.dat, built from
                          up to 32 lines (or a prebuilt file), signed with the
                          server key in --data
  invite create [--days N] [--data DIR]
                          print an invite code for an --invite-only server

--server defaults to $H2_LIVE_SERVER or http://127.0.0.1:8074; --data to
~/Library/Application Support/h2recomp/live/server.";

pub fn run(args: Vec<String>) -> Result<()> {
    let Some((sub, rest)) = args.split_first() else {
        bail!("{USAGE}")
    };
    let rest = Args(rest.to_vec());
    match (sub.as_str(), rest.0.first().map(String::as_str)) {
        ("serve", _) => serve(rest),
        ("account", Some("create")) => account_create(rest.shift()),
        ("account", Some("list")) => account_list(rest.shift()),
        ("motd", Some("set")) => motd_set(rest.shift()),
        ("invite", Some("create")) => invite_create(rest.shift()),
        ("-h" | "--help", _) => bail!("{USAGE}"),
        _ => bail!("unknown h2 live command {:?}\n\n{USAGE}", args.join(" ")),
    }
}

/// A tiny flag parser: `take` removes `--flag value`, `flag` removes a
/// bare switch, and what is left is positional.
struct Args(Vec<String>);

impl Args {
    fn shift(mut self) -> Self {
        self.0.remove(0);
        self
    }

    fn take(&mut self, name: &str) -> Result<Option<String>> {
        let Some(i) = self.0.iter().position(|a| a == name) else {
            return Ok(None);
        };
        if i + 1 >= self.0.len() {
            bail!("{name} needs a value");
        }
        let value = self.0.remove(i + 1);
        self.0.remove(i);
        Ok(Some(value))
    }

    fn flag(&mut self, name: &str) -> bool {
        let found = self.0.iter().position(|a| a == name);
        found.map(|i| self.0.remove(i)).is_some()
    }

    fn positional(self) -> Result<Vec<String>> {
        if let Some(bad) = self.0.iter().find(|a| a.starts_with("--")) {
            if bad == "--help" {
                bail!("{USAGE}");
            }
            bail!("unexpected {bad}\n\n{USAGE}");
        }
        Ok(self.0)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn live_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .context("HOME is not set; nowhere to keep live data")?;
    Ok(PathBuf::from(home).join("Library/Application Support/h2recomp/live"))
}

fn data_dir(flag: Option<String>) -> Result<PathBuf> {
    flag.map(PathBuf::from)
        .map_or_else(|| Ok(live_dir()?.join("server")), Ok)
}

fn serve(mut args: Args) -> Result<()> {
    let db = args
        .take("--db")?
        .or_else(|| env("H2_LIVE_DB"))
        .unwrap_or_else(|| live_server::DEFAULT_DB.into());
    let listen = args
        .take("--listen")?
        .unwrap_or_else(|| live_server::DEFAULT_LISTEN.into());
    let listen = listen.parse().with_context(|| {
        format!(
            "bad --listen address (want ip:port like {})",
            live_server::DEFAULT_LISTEN
        )
    })?;
    let data_dir = data_dir(args.take("--data")?)?;
    let id = args.take("--id")?;
    let tls = match (args.take("--tls-cert")?, args.take("--tls-key")?) {
        (Some(cert), Some(key)) => Some((cert.into(), key.into())),
        (None, None) => None,
        _ => bail!("--tls-cert and --tls-key go together"),
    };
    let policy = live_server::Policy {
        private: args.flag("--private"),
        invite_only: args.flag("--invite-only"),
        ..live_server::Policy::default()
    };
    let insecure_http = args.flag("--insecure-http");
    if !args.positional()?.is_empty() {
        bail!("serve takes no positional arguments\n\n{USAGE}");
    }
    let config = live_server::Config {
        db,
        listen,
        data_dir,
        id,
        policy,
        tls,
        insecure_http,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio")?;
    rt.block_on(live_server::serve(config))
}

/// A service, found from its base URL.
struct Client {
    base: String,
    api: String,
    discovery: Discovery,
    agent: ureq::Agent,
}

impl Client {
    fn connect(flag: Option<String>) -> Result<Self> {
        let base = flag
            .or_else(|| env("H2_LIVE_SERVER"))
            .unwrap_or_else(|| "http://127.0.0.1:8074".into());
        let base = base.trim_end_matches('/').to_string();
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .build();
        let url = format!("{base}/.well-known/h2live.json");
        let discovery: Discovery = serde_json::from_str(&call(agent.get(&url), None)?)
            .with_context(|| format!("{url} is not an h2 live discovery document"))?;
        let api = resolve(&base, &discovery.api);
        Ok(Client {
            base,
            api,
            discovery,
            agent,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    /// Adds a bearer token, refusing to send one in the clear.
    fn authorized(&self, req: ureq::Request, token: String) -> Result<ureq::Request> {
        if !bearer_allowed(req.url()) {
            bail!(
                "refusing to send a token over plain HTTP to {}; use https",
                self.base
            );
        }
        Ok(req.set("authorization", &format!("Bearer {token}")))
    }
}

/// The well-known `api` may be relative to the server's base URL.
fn resolve(base: &str, api: &str) -> String {
    let api = api.trim_end_matches('/');
    if api.contains("://") {
        api.to_string()
    } else if let Some(path) = api.strip_prefix('/') {
        // Relative to the origin, not the base path.
        let origin_end = base.find("://").map_or(0, |i| i + 3);
        let origin = &base[..base[origin_end..]
            .find('/')
            .map_or(base.len(), |i| origin_end + i)];
        format!("{origin}/{path}")
    } else {
        format!("{base}/{api}")
    }
}

/// One request; a non-2xx answer becomes an error with the service's
/// message.
fn call(req: ureq::Request, body: Option<&[u8]>) -> Result<String> {
    let result = match body {
        Some(b) => req.send_bytes(b),
        None => req.call(),
    };
    match result {
        Ok(resp) => Ok(resp.into_string()?),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<ErrorBody>(&text)
                .map(|e| format!("{} ({})", e.message, e.error))
                .unwrap_or(text);
            bail!("HTTP {code} from the live service: {detail}")
        }
        Err(ureq::Error::Transport(t)) => bail!("reaching the live service: {t}"),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn account_create(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let invite = args.take("--invite")?;
    let [gamertag] = args.positional()?.try_into().map_err(|_| {
        anyhow::anyhow!("account create takes one gamertag (quote it if it has spaces)\n\n{USAGE}")
    })?;
    let tag = Gamertag::new(&gamertag)
        .with_context(|| format!("{gamertag:?} is not a valid gamertag"))?;
    let client = Client::connect(server)?;
    let key = KeyPair::generate();
    let req = client
        .agent
        .post(&client.url("/accounts"))
        .set("content-type", "application/jose");
    let body = AccountRequest::sign(&key, tag.as_str(), invite);
    let created: CreatedAccount = serde_json::from_str(&call(req, Some(body.as_bytes()))?)?;
    let path = live_dir()?.join("accounts.json");
    store_local(&path, &created, &client, &key).with_context(|| {
        format!(
            "{} was created but its key could not be saved; the account is unusable",
            created.gamertag
        )
    })?;
    println!(
        "created {} ({}) as {}",
        created.gamertag, created.xuid, created.id
    );
    println!("key stored in {}", path.display());
    Ok(())
}

/// Upserts the account (by xuid) into the local accounts file and leaves
/// it mode 0600 (an older, looser file is tightened). Other entries are
/// kept as they are.
fn store_local(
    path: &Path,
    account: &CreatedAccount,
    client: &Client,
    key: &KeyPair,
) -> Result<()> {
    let mut entries: Vec<Value> = match fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let xuid = serde_json::to_value(account.xuid)?;
    entries.retain(|e| e.get("xuid") != Some(&xuid));
    entries.push(serde_json::json!({
        "xuid": account.xuid,
        "gamertag": account.gamertag,
        "id": account.id,
        "server": client.base,
        "server_id": client.discovery.id,
        "kid": key.kid(),
        "key": key.secret_b64(),
    }));
    write_private(
        path,
        format!("{}\n", serde_json::to_string_pretty(&entries)?).as_bytes(),
    )
}

fn account_list(mut args: Args) -> Result<()> {
    let client = Client::connect(args.take("--server")?)?;
    args.positional()?;
    let mut cursor: Option<String> = None;
    let mut first = true;
    loop {
        let mut req = client
            .agent
            .get(&client.url("/accounts"))
            .query("limit", "1000");
        if let Some(c) = &cursor {
            req = req.query("cursor", c);
        }
        let page: Page<AccountSummary> = serde_json::from_str(&call(req, None)?)?;
        if first && page.items.is_empty() {
            println!("no accounts");
        }
        for a in &page.items {
            if first {
                println!("{:<18} {:<16} {:<7} created", "xuid", "gamertag", "online");
                first = false;
            }
            let online = if a.online { "yes" } else { "no" };
            println!(
                "{:<18} {:<16} {online:<7} {}",
                a.xuid.to_string(),
                a.gamertag.as_str(),
                a.created_at
            );
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(()),
        }
    }
}

/// The operator: this machine's server identity, checked against the
/// server it is about to talk to.
fn operator(data: Option<String>, client: &Client) -> Result<ServerIdentity> {
    let dir = data_dir(data)?;
    let identity = ServerIdentity::load(&dir)
        .with_context(|| format!("loading the server identity in {}", dir.display()))?;
    let theirs = client
        .discovery
        .keys
        .iter()
        .any(|k| k.kid.as_deref() == Some(identity.key.kid()));
    if identity.id != client.discovery.id || !theirs {
        bail!(
            "{} holds the key of server {:?}, not of {} ({:?})",
            dir.display(),
            identity.id,
            client.base,
            client.discovery.id
        );
    }
    Ok(identity)
}

fn motd_set(mut args: Args) -> Result<()> {
    let client_flag = args.take("--server")?;
    let data = args.take("--data")?;
    let header = args.take("--header")?;
    let file = args.take("--file")?;
    let lines = args.positional()?;
    let bytes = match (file, lines.is_empty()) {
        (Some(path), true) => {
            if header.is_some() {
                bail!("--header builds a file; it does not go with --file");
            }
            fs::read(&path).with_context(|| format!("reading {path}"))?
        }
        (None, false) => Motd {
            header: header.unwrap_or_default(),
            messages: lines,
        }
        .build()?,
        _ => bail!("motd set takes message lines or --file, not both or neither\n\n{USAGE}"),
    };
    if bytes.len() > live_server::MAX_TITLE_FILE {
        bail!(
            "the file is {} bytes; title files are at most {} MiB",
            bytes.len(),
            live_server::MAX_TITLE_FILE >> 20
        );
    }
    let client = Client::connect(client_flag)?;
    let me = operator(data, &client)?;
    let token = issue_token(&me.key, &me.author(), &me.id, lxm::TITLES_WRITE, now());
    let req = client.agent.put(&client.url(&format!(
        "/titles/{HALO_2_TITLE_ID}/files/{HALO_2_MOTD_FILE}"
    )));
    let req = client.authorized(req.set("content-type", "application/octet-stream"), token)?;
    let published: PublishedFile = serde_json::from_str(&call(req, Some(&bytes))?)?;
    println!(
        "published {} ({} bytes, sha256 {})",
        published.name, published.size, published.sha256
    );
    Ok(())
}

fn invite_create(mut args: Args) -> Result<()> {
    let days: i64 = args
        .take("--days")?
        .map_or(Ok(7), |d| d.parse())
        .context("--days must be a whole number")?;
    let dir = data_dir(args.take("--data")?)?;
    args.positional()?;
    let me = ServerIdentity::load(&dir)
        .with_context(|| format!("loading the server identity in {}", dir.display()))?;
    println!("{}", Invite::issue(&me.key, &me.id, now() + days * 86_400));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Args {
        Args(s.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn flags_and_positionals() {
        let mut a = args(&["Hello", "--header", "Hi", "World", "--private"]);
        assert_eq!(a.take("--header").unwrap().as_deref(), Some("Hi"));
        assert!(a.flag("--private"));
        assert!(!a.flag("--private"));
        assert_eq!(a.positional().unwrap(), ["Hello", "World"]);
        assert!(args(&["--header"]).take("--header").is_err());
        assert!(args(&["x", "--bogus"]).positional().is_err());
    }

    #[test]
    fn well_known_api_resolution() {
        assert_eq!(
            resolve("http://127.0.0.1:8074", "/v1"),
            "http://127.0.0.1:8074/v1"
        );
        assert_eq!(
            resolve("https://h.example/live", "/v1"),
            "https://h.example/v1"
        );
        assert_eq!(
            resolve("https://h.example/live", "v1"),
            "https://h.example/live/v1"
        );
        assert_eq!(
            resolve("http://a", "https://b.example/api/"),
            "https://b.example/api"
        );
    }
}
