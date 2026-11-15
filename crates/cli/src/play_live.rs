//! `h2 play --live`: the game signed in to a Live service. Either an
//! embedded server on a background runtime (Postgres from `$H2_LIVE_DB`,
//! else the in-memory store) or `--live-server URL`; an account for that
//! server is found in, or added to, the local accounts file, and the game
//! is booted with `boot::LiveConfig` pointing at it (docs/live.md).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use live_proto::Gamertag;

use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Client, status_of};

/// The `--live*` flags of `h2 play`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveOpts {
    pub enabled: bool,
    /// `--live-server`: use this service instead of an embedded one.
    pub server: Option<String>,
    /// `--live-listen`: where the embedded server binds (default
    /// 127.0.0.1 on a free port).
    pub listen: Option<SocketAddr>,
    /// `--gamertag`: the account to play as (made if missing).
    pub gamertag: Option<String>,
}

impl LiveOpts {
    /// Takes `flag` (and its value) if it is one of ours.
    pub fn take(&mut self, flag: &str, value: &mut dyn FnMut() -> Result<String>) -> Result<bool> {
        match flag {
            "--live" => self.enabled = true,
            "--live-server" => {
                self.enabled = true;
                self.server = Some(value()?);
            }
            "--live-listen" => {
                self.enabled = true;
                let v = value()?;
                self.listen = Some(
                    v.parse()
                        .with_context(|| format!("bad --live-listen {v:?} (want ip:port)"))?,
                );
            }
            "--gamertag" => {
                let v = value()?;
                Gamertag::new(&v).map_err(|e| anyhow::anyhow!("--gamertag {v:?}: {e}"))?;
                self.gamertag = Some(v);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn check(&self) -> Result<()> {
        if self.gamertag.is_some() && !self.enabled {
            bail!("--gamertag goes with --live or --live-server");
        }
        if self.server.is_some() && self.listen.is_some() {
            bail!("--live-listen is for the embedded server; --live-server uses another");
        }
        Ok(())
    }
}

/// A running live session: what to boot the game with, and the embedded
/// server (stopped on drop) if there is one.
pub struct Session {
    pub config: boot::LiveConfig,
    pub account: LocalAccount,
    pub server: String,
    embedded: Option<live_server::Embedded>,
}

impl Session {
    pub fn store(&self) -> Option<&'static str> {
        self.embedded.as_ref().map(|e| e.store)
    }
}

/// Where a session keeps things: `accounts.json` (shared with `h2 live
/// account`), `server/` (the embedded server's identity) and
/// `play-accounts.json`, the one account handed to the game.
pub struct Places {
    pub accounts: PathBuf,
    pub server_data: PathBuf,
    pub session_accounts: PathBuf,
}

impl Places {
    pub fn under(live_dir: &Path) -> Self {
        Places {
            accounts: live_dir.join("accounts.json"),
            server_data: live_dir.join("server"),
            session_accounts: live_dir.join("play-accounts.json"),
        }
    }
}

/// Starts (or finds) the server and the account. `db` is the embedded
/// server's database URL (or [`live_server::MEMORY_DB`]); `default_name`
/// the OS user name a new account is named after without `--gamertag`.
pub fn start(
    opts: &LiveOpts,
    db: &str,
    places: &Places,
    site: live_server::http::Site,
    default_name: &str,
) -> Result<Session> {
    let embedded = match &opts.server {
        Some(_) => None,
        None => {
            let listen = opts
                .listen
                .unwrap_or_else(|| "127.0.0.1:0".parse().expect("an address"));
            Some(live_server::start_embedded(
                db,
                listen,
                &places.server_data,
                site,
            )?)
        }
    };
    let url = embedded.as_ref().map_or_else(
        || opts.server.clone().unwrap_or_default(),
        |e| e.url.clone(),
    );
    let client = Client::at(&url)?;
    let (account, created) = bootstrap(
        &client,
        &places.accounts,
        opts.gamertag.as_deref(),
        default_name,
    )?;
    if created {
        eprintln!(
            "live: created {} ({}) on {}",
            account.gamertag, account.xuid, client.discovery.id
        );
    }
    live_accounts::save(&places.session_accounts, std::slice::from_ref(&account))?;
    Ok(Session {
        config: boot::LiveConfig {
            enabled: true,
            accounts_path: Some(places.session_accounts.clone()),
            server_url: Some(client.base.clone()),
        },
        account,
        server: client.base,
        embedded,
    })
}

/// The account to play as on `client`'s server: the first local one (named
/// `want`, if given) whose key the server still lists as current, else a
/// new one, named `want` or after `default_name` (numbered if taken).
/// Returns it and whether it was made now.
pub fn bootstrap(
    client: &Client,
    accounts_path: &Path,
    want: Option<&str>,
    default_name: &str,
) -> Result<(LocalAccount, bool)> {
    let mut accounts = live_accounts::load(accounts_path)?;
    let on_server = |a: &&LocalAccount| a.is_on(&client.base, &client.discovery.id);
    let named = |a: &&LocalAccount| want.is_none_or(|w| a.gamertag.eq_ignore_ascii_case(w));
    for a in accounts.iter().filter(on_server).filter(named) {
        let Ok(key) = a.key_pair() else { continue };
        let keys = match client.keys(a.xuid) {
            Ok(k) => k,
            Err(e) if status_of(&e) == Some(404) => continue,
            Err(e) => return Err(e),
        };
        if keys
            .keys
            .iter()
            .any(|k| k.kid == key.kid() && k.valid_to.is_none())
        {
            return Ok((a.clone(), false));
        }
    }
    let base = want.map_or_else(
        || live_accounts::gamertag_from(default_name),
        str::to_string,
    );
    for n in 1..=20 {
        let tag = if n == 1 {
            base.clone()
        } else {
            live_accounts::numbered(&base, n)
        };
        match client.create_account(&tag, None) {
            Ok((created, key)) => {
                let account = LocalAccount::new(
                    created.xuid,
                    created.gamertag.as_str(),
                    &client.base,
                    &client.discovery,
                    &key,
                );
                // An entry of the same name here is stale: the server no
                // longer has it.
                accounts.retain(|a| {
                    !(a.is_on(&client.base, &client.discovery.id)
                        && a.gamertag.eq_ignore_ascii_case(&tag))
                });
                live_accounts::upsert(&mut accounts, account.clone());
                live_accounts::save(accounts_path, &accounts)?;
                return Ok((account, true));
            }
            Err(e) if status_of(&e) == Some(409) && want.is_none() => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("creating {tag:?} on {}", client.base));
            }
        }
    }
    bail!(
        "every gamertag from {base:?} to {:?} is taken; pass --gamertag",
        live_accounts::numbered(&base, 20)
    )
}

/// The game art in `dir`, extracted from `disc` first unless it already
/// holds this disc's art. Any failure is a warning: the web app keeps its
/// own art.
pub fn game_art(disc: &Path, dir: &Path) -> Option<live_server::art::GameArt> {
    let disc = match crate::live_art::Disc::scan(disc) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("live: no game art ({e:#})");
            return None;
        }
    };
    if crate::live_art::cached(dir, &disc.key).is_none() {
        eprintln!(
            "live: extracting game art from the disc into {} (once per disc)",
            dir.display()
        );
        if let Err(e) = crate::live_art::extract(&disc, dir) {
            eprintln!("live: game art extraction failed ({e:#}); the web app keeps its own art");
            return None;
        }
    }
    live_server::art::GameArt::found(dir, false)
}

/// The OS user name, for a default gamertag.
pub fn os_user() -> String {
    ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2-play-live-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn parse(args: &[&str]) -> Result<LiveOpts> {
        let mut opts = LiveOpts::default();
        let mut it = args.iter().map(|s| s.to_string());
        while let Some(a) = it.next() {
            let mut value = || it.next().context("needs a value");
            assert!(opts.take(&a, &mut value)?, "{a} is ours");
        }
        opts.check()?;
        Ok(opts)
    }

    #[test]
    fn flags() {
        assert_eq!(
            parse(&["--live"]).unwrap(),
            LiveOpts {
                enabled: true,
                ..LiveOpts::default()
            }
        );
        let o = parse(&[
            "--live-server",
            "https://h.example",
            "--gamertag",
            "Sneaky Pete",
        ])
        .unwrap();
        assert!(
            o.enabled
                && o.server.as_deref() == Some("https://h.example")
                && o.gamertag.as_deref() == Some("Sneaky Pete")
        );
        assert_eq!(
            parse(&["--live-listen", "127.0.0.1:9000"]).unwrap().listen,
            Some("127.0.0.1:9000".parse().unwrap())
        );
        assert!(parse(&["--gamertag", "Pete"]).is_err(), "needs --live");
        assert!(
            parse(&["--live", "--gamertag", "9lives"]).is_err(),
            "bad gamertag"
        );
        assert!(parse(&["--live-listen", "nope"]).is_err());
        assert!(parse(&["--live-server", "http://a", "--live-listen", "127.0.0.1:1"]).is_err());
        assert!(parse(&["--live-server"]).is_err());
        assert!(
            !LiveOpts::default()
                .take("--out", &mut || Ok(String::new()))
                .unwrap()
        );
    }

    #[test]
    fn an_embedded_session_makes_then_reuses_an_account() {
        let dir = temp("session");
        let places = Places::under(&dir);
        let opts = LiveOpts {
            enabled: true,
            ..LiveOpts::default()
        };
        let session = start(
            &opts,
            live_server::MEMORY_DB,
            &places,
            Default::default(),
            "test.user",
        )
        .unwrap();
        assert_eq!(session.store(), Some("memory"));
        assert_eq!(session.account.gamertag, "test user");
        assert_eq!(
            session.config.server_url.as_deref(),
            Some(session.server.as_str())
        );
        assert_eq!(
            session.config.accounts_path.as_deref(),
            Some(places.session_accounts.as_path())
        );
        let handed = live_accounts::load(&places.session_accounts).unwrap();
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0].xuid, session.account.xuid);

        // The same server again: the account is found, not made.
        let client = Client::at(&session.server).unwrap();
        let (again, created) = bootstrap(&client, &places.accounts, None, "test.user").unwrap();
        assert!(!created);
        assert_eq!(again.xuid, session.account.xuid);
        // Another name is made beside it; a taken default is numbered.
        let (other, created) = bootstrap(&client, &places.accounts, Some("Other"), "x").unwrap();
        assert!(created && other.gamertag == "Other");
        let elsewhere = dir.join("elsewhere.json");
        let (numbered, _) = bootstrap(&client, &elsewhere, None, "test.user").unwrap();
        assert_eq!(numbered.gamertag, "test user2");
        // A chosen name someone else has is an error.
        assert!(bootstrap(&client, &elsewhere, Some("Other"), "x").is_err());
        assert_eq!(live_accounts::load(&places.accounts).unwrap().len(), 2);

        // Stopping the session stops the server.
        let addr = session.server.trim_start_matches("http://").to_string();
        drop(session);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(std::net::TcpStream::connect(&addr).is_err());

        // A fresh in-memory server no longer has the account: a new one
        // replaces the stale entry.
        let session = start(
            &opts,
            live_server::MEMORY_DB,
            &places,
            Default::default(),
            "test.user",
        )
        .unwrap();
        assert_eq!(session.account.gamertag, "test user");
        let accounts = live_accounts::load(&places.accounts).unwrap();
        assert_eq!(
            accounts
                .iter()
                .filter(|a| a.gamertag == "test user")
                .count(),
            1
        );
        drop(session);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
