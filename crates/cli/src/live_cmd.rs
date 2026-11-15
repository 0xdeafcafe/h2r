//! `h2 live ...`: run the Xbox Live replacement service and use it
//! (docs/live.md §4.2, docs/live-security.md).
//!
//! Host data lives beside the other h2recomp data
//! (`~/Library/Application Support/h2recomp/live`):
//!
//! - `accounts.json` (0600): the accounts made or imported here, each with
//!   its private key ([`crate::live_accounts`]). The game-side HLE reads
//!   `xuid` and `gamertag` from it.
//! - `server/` (0700): `h2 live serve`'s identity (key and id). The CLI
//!   signs operator requests (`motd set`, `invite create`) with it, so they
//!   work on the machine the server runs on.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use live_proto::motd::Motd;
use live_proto::sign::{AccountRequest, KeyPair, issue_token};
use live_proto::{
    AccountKeys, AccountSummary, CreatedAccount, Discovery, ErrorBody, Gamertag, HALO_2_MOTD_FILE,
    HALO_2_TITLE_ID, Page, PublishedFile, Xuid, bearer_allowed, lxm,
};
use live_server::ServerIdentity;
use serde::de::DeserializeOwned;

use crate::live_accounts::{self, LocalAccount};

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
  account import <file> [--server URL]
                          add an account the web app made (its downloaded
                          {xuid, gamertag, server, key}) to accounts.json,
                          once the server confirms the key is the account's
  account list [--server URL]
  motd set <line>... [--header TEXT] [--server URL] [--data DIR]
  motd set --file PATH [--server URL] [--data DIR]
                          publish Halo 2's message_of_the_day.dat, built from
                          up to 32 lines (or a prebuilt file), signed with the
                          server key in --data
  invite create [--days N] [--server URL] [--data DIR]
                          print an invite code for an --invite-only server
                          (POST /v1/invites, signed with the server key)
  q <what> [--server URL] [--json|--jsonl]
                          query the service; `h2 live q --help` lists what

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
        ("account", Some("import")) => account_import(rest.shift()),
        ("account", Some("list")) => account_list(rest.shift()),
        ("q", _) => crate::live_query::run(rest),
        ("motd", Some("set")) => motd_set(rest.shift()),
        ("invite", Some("create")) => invite_create(rest.shift()),
        ("-h" | "--help", _) => bail!("{USAGE}"),
        _ => bail!("unknown h2 live command {:?}\n\n{USAGE}", args.join(" ")),
    }
}

/// A tiny flag parser: `take` removes `--flag value`, `flag` removes a
/// bare switch, and what is left is positional.
pub(crate) struct Args(pub(crate) Vec<String>);

impl Args {
    pub(crate) fn shift(mut self) -> Self {
        self.0.remove(0);
        self
    }

    pub(crate) fn take(&mut self, name: &str) -> Result<Option<String>> {
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

    pub(crate) fn flag(&mut self, name: &str) -> bool {
        let found = self.0.iter().position(|a| a == name);
        found.map(|i| self.0.remove(i)).is_some()
    }

    pub(crate) fn positional(self) -> Result<Vec<String>> {
        self.positional_for(USAGE)
    }

    pub(crate) fn positional_for(self, usage: &str) -> Result<Vec<String>> {
        if let Some(bad) = self.0.iter().find(|a| a.starts_with("--")) {
            if bad == "--help" {
                bail!("{usage}");
            }
            bail!("unexpected {bad}\n\n{usage}");
        }
        Ok(self.0)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

pub(crate) fn live_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .context("HOME is not set; nowhere to keep live data")?;
    Ok(PathBuf::from(home).join("Library/Application Support/h2recomp/live"))
}

pub(crate) fn data_dir(flag: Option<String>) -> Result<PathBuf> {
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
        web: live_server::web_dir(),
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting tokio")?;
    rt.block_on(live_server::serve(config))
}

/// A service, found from its base URL.
pub(crate) struct Client {
    pub(crate) base: String,
    pub(crate) api: String,
    pub(crate) discovery: Discovery,
    pub(crate) agent: ureq::Agent,
}

/// `--server`, else `$H2_LIVE_SERVER`, else the local default.
pub(crate) fn server_url(flag: Option<String>) -> String {
    flag.or_else(|| env("H2_LIVE_SERVER"))
        .unwrap_or_else(|| "http://127.0.0.1:8074".into())
}

impl Client {
    pub(crate) fn connect(flag: Option<String>) -> Result<Self> {
        Self::at(&server_url(flag))
    }

    pub(crate) fn at(base: &str) -> Result<Self> {
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

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    /// `GET` an API path, parsed.
    pub(crate) fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut req = self.agent.get(&self.url(path));
        for (k, v) in query {
            req = req.query(k, v);
        }
        let text = call(req, None)?;
        serde_json::from_str(&text).with_context(|| format!("{path}: an unexpected reply"))
    }

    /// The keys of an account, by xuid.
    pub(crate) fn keys(&self, xuid: Xuid) -> Result<AccountKeys> {
        self.get(&format!("/accounts/by-xuid/{xuid}/keys"), &[])
    }

    /// Registers a new account with a fresh key.
    pub(crate) fn create_account(
        &self,
        gamertag: &str,
        invite: Option<String>,
    ) -> Result<(CreatedAccount, KeyPair)> {
        let key = KeyPair::generate();
        let req = self
            .agent
            .post(&self.url("/accounts"))
            .set("content-type", "application/jose");
        let body = AccountRequest::sign(&key, gamertag, invite, &self.discovery.id, now());
        let created = serde_json::from_str(&call(req, Some(body.as_bytes()))?)?;
        Ok((created, key))
    }

    /// Adds a bearer token, refusing to send one in the clear.
    pub(crate) fn authorized(&self, req: ureq::Request, token: String) -> Result<ureq::Request> {
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

/// A non-2xx answer: the error [`call`] returns, so callers can match on
/// `code` (`err.downcast_ref::<HttpStatus>()`).
#[derive(Debug)]
pub(crate) struct HttpStatus {
    pub(crate) code: u16,
    detail: String,
}

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HTTP {} from the live service: {}",
            self.code, self.detail
        )
    }
}

impl std::error::Error for HttpStatus {}

/// The status of a failed [`call`], if the server answered.
pub(crate) fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.chain()
        .find_map(|c| c.downcast_ref::<HttpStatus>())
        .map(|h| h.code)
}

/// One request; a non-2xx answer becomes an [`HttpStatus`] error with the
/// service's message.
pub(crate) fn call(req: ureq::Request, body: Option<&[u8]>) -> Result<String> {
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
            Err(HttpStatus { code, detail }.into())
        }
        Err(ureq::Error::Transport(t)) => bail!("reaching the live service: {t}"),
    }
}

pub(crate) fn now() -> i64 {
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
    let (created, key) = client.create_account(tag.as_str(), invite)?;
    let path = accounts_path()?;
    let account = LocalAccount::new(
        created.xuid,
        created.gamertag.as_str(),
        &client.base,
        &client.discovery,
        &key,
    );
    store(&path, account).with_context(|| {
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

pub(crate) fn accounts_path() -> Result<PathBuf> {
    Ok(live_dir()?.join("accounts.json"))
}

/// Upserts the account (by xuid) into the local accounts file; other
/// entries are kept as they are.
fn store(path: &Path, account: LocalAccount) -> Result<()> {
    let mut accounts = live_accounts::load(path)?;
    live_accounts::upsert(&mut accounts, account);
    live_accounts::save(path, &accounts)
}

fn account_import(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let [file] = args
        .positional()?
        .try_into()
        .map_err(|_| anyhow::anyhow!("account import takes one file\n\n{USAGE}"))?;
    let path = accounts_path()?;
    for account in import(Path::new(&file), server, &path)? {
        println!(
            "imported {} ({}) from {}",
            account.gamertag, account.xuid, account.server
        );
    }
    println!("accounts in {}", path.display());
    Ok(())
}

/// Validates every entry of `file` with its server (or `server`), then
/// merges them all into `accounts` or none.
pub(crate) fn import(
    file: &Path,
    server: Option<String>,
    accounts: &Path,
) -> Result<Vec<LocalAccount>> {
    let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let entries = live_accounts::parse_import(&bytes)?;
    let mut all = live_accounts::load(accounts)?;
    let mut added = Vec::new();
    for entry in entries {
        let client = Client::at(server.as_deref().unwrap_or(&entry.server))?;
        let keys = client
            .keys(entry.xuid)
            .with_context(|| format!("looking up xuid {} on {}", entry.xuid, client.base))?;
        let mut checked = live_accounts::check_import(entry, &client.discovery, &keys)?;
        checked.server = client.base.clone();
        live_accounts::merge(&mut all, checked.clone())?;
        added.push(checked);
    }
    live_accounts::save(accounts, &all)?;
    Ok(added)
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
    let server = args.take("--server")?;
    let data = args.take("--data")?;
    args.positional()?;
    let client = Client::connect(server)?;
    let me = operator(data, &client)?;
    let token = issue_token(&me.key, &me.author(), &me.id, lxm::INVITES_WRITE, now());
    let req = client
        .agent
        .post(&client.url("/invites"))
        .query("days", &days.to_string());
    let req = client.authorized(req, token)?;
    let reply: serde_json::Value = serde_json::from_str(&call(req, Some(b""))?)?;
    let invite = reply["invite"]
        .as_str()
        .context("the server's reply has no invite")?;
    println!("{invite}");
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
    fn importing_a_browser_account() {
        let dir = std::env::temp_dir().join(format!("h2-live-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir.join("server"),
            None,
        )
        .unwrap();
        // What the web app does: make a key, sign up, download the entry.
        let client = Client::at(&server.url).unwrap();
        let (created, key) = client.create_account("Web Pete", None).unwrap();
        let entry = serde_json::json!({
            "xuid": created.xuid, "gamertag": "Web Pete", "server": server.url,
            "key": key.private_jwk(),
        });
        let file = dir.join("web-pete.json");
        fs::write(&file, entry.to_string()).unwrap();
        let accounts = dir.join("accounts.json");
        let added = import(&file, None, &accounts).unwrap();
        assert_eq!(added[0].id.as_deref(), Some(created.id.as_str()));
        let stored = live_accounts::load(&accounts).unwrap();
        assert_eq!(stored[0].key_pair().unwrap().kid(), key.kid());
        // Twice is a duplicate; another key is not the account's.
        assert!(import(&file, None, &accounts).is_err());
        let mut forged = entry.clone();
        forged["key"] = serde_json::to_value(KeyPair::generate().private_jwk()).unwrap();
        fs::write(&file, forged.to_string()).unwrap();
        let other = dir.join("other.json");
        let err = import(&file, None, &other).unwrap_err();
        assert!(format!("{err:#}").contains("not a current key"), "{err:#}");
        assert!(!other.exists(), "nothing written for a refused import");
        // An xuid the server does not know.
        let mut unknown = entry;
        unknown["xuid"] = "0009ffffffffffff".into();
        fs::write(&file, unknown.to_string()).unwrap();
        assert_eq!(
            status_of(&import(&file, None, &other).unwrap_err()),
            Some(404)
        );
        drop(server);
        fs::remove_dir_all(&dir).unwrap();
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
