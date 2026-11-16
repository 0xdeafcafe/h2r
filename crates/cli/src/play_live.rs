//! `h2 play`'s Live flags (docs/live/live.md "Playing on Live"). With none
//! the game plays offline and nothing is contacted. `--live` signs in to
//! [`DEFAULT_LIVE_SERVER`] (or `$H2_LIVE_SERVER`), `--live-server URL` to
//! that service, and `--live-local` to an embedded server on a background
//! runtime (Postgres from `$H2_LIVE_DB`, else the file store in
//! `server/log`), which `--live-listen` and `--live-tunnel` configure. An
//! account for the server is found in, or added to, the local accounts
//! file, and the game is booted with `boot::LiveConfig` pointing at it.
//! The tvOS and iOS apps are on by default instead (`live_hle::platform`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use live_proto::{Gamertag, ServerOverride};

use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Client, status_of};

/// The Live service `h2 play --live` signs in to: [`live_proto::DEFAULT_SERVER`],
/// shared with the app builds.
pub const DEFAULT_LIVE_SERVER: &str = live_proto::DEFAULT_SERVER;

/// The `--live*` flags of `h2 play`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveOpts {
    /// Any Live flag: `--live`, `--live-server`, `--live-local` (or one
    /// that implies it).
    pub enabled: bool,
    /// `--live-local`: run the server in this process. `--live-listen` and
    /// `--live-tunnel` imply it.
    pub local: bool,
    /// `--offline` (or `--no-live`): no Live, as with no flag. Kept for
    /// scripts and for symmetry with the app builds.
    pub offline: bool,
    /// `--live-server`: this service. With `--live` alone it is filled in
    /// from `$H2_LIVE_SERVER` or [`DEFAULT_LIVE_SERVER`] ([`choose`]).
    pub server: Option<String>,
    /// `--live-listen`: where the embedded server binds (default
    /// 127.0.0.1 on a free port).
    pub listen: Option<SocketAddr>,
    /// `--gamertag`: the account to play as (made if missing).
    pub gamertag: Option<String>,
    /// `--live-tunnel` (alias `--tunnel`), with `--live-tunnel-name NAME`
    /// (`--tunnel-name`) and `--live-tunnel-url https://HOST` (or
    /// `--hostname HOST`): publish the embedded server through cloudflared
    /// so friends can join it (docs/live/live-hosting.md).
    pub tunnel: bool,
    pub tunnel_name: Option<String>,
    pub hostname: Option<String>,
    /// `--no-tunnel-setup`: run a named tunnel as it is, without making it
    /// or its DNS route first.
    pub no_tunnel_setup: bool,
    /// Where cloudflared's login certificate is looked for; `None`: as
    /// cloudflared does (tests set a directory of their own).
    pub origin_cert: Option<crate::tunnel::OriginCert>,
    /// The cloudflared to run; `None`: `$H2_CLOUDFLARED`, else PATH (tests
    /// set a fake).
    pub cloudflared: Option<PathBuf>,
}

impl LiveOpts {
    /// Takes `flag` (and its value) if it is one of ours.
    pub fn take(&mut self, flag: &str, value: &mut dyn FnMut() -> Result<String>) -> Result<bool> {
        match flag {
            "--live" => self.enabled = true,
            "--live-local" => self.embedded(),
            "--offline" | "--no-live" => self.offline = true,
            "--live-server" => {
                self.enabled = true;
                self.server = Some(value()?);
            }
            "--live-listen" => {
                self.embedded();
                let v = value()?;
                self.listen = Some(
                    v.parse()
                        .with_context(|| format!("bad --live-listen {v:?} (want ip:port)"))?,
                );
            }
            "--live-tunnel" | "--tunnel" => {
                self.embedded();
                self.tunnel = true;
            }
            "--live-tunnel-name" | "--tunnel-name" => {
                self.embedded();
                self.tunnel_name = Some(value()?);
            }
            "--live-tunnel-url" => {
                self.hostname = Some(crate::tunnel::hostname_from_url(&value()?)?);
            }
            "--hostname" => self.hostname = Some(value()?),
            "--no-tunnel-setup" => self.no_tunnel_setup = true,
            "--gamertag" => {
                let v = value()?;
                Gamertag::new(&v).map_err(|e| anyhow::anyhow!("--gamertag {v:?}: {e}"))?;
                self.gamertag = Some(v);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn embedded(&mut self) {
        self.enabled = true;
        self.local = true;
    }

    pub fn check(&self) -> Result<()> {
        if self.offline && self.enabled {
            bail!(
                "--offline plays without Live; drop --live, --live-local, --live-server or \
                 --live-tunnel"
            );
        }
        if self.gamertag.is_some() && !self.enabled {
            bail!(
                "--gamertag is the Live account to play as; it goes with --live, --live-server \
                 or --live-local"
            );
        }
        if self.server.is_some() && self.listen.is_some() {
            bail!(
                "--live-listen is for the embedded server (--live-local); --live-server uses another"
            );
        }
        if self.hostname.is_some() && self.tunnel_name.is_none() {
            bail!(
                "--live-tunnel-url (--hostname) goes with --live-tunnel-name: a quick tunnel's \
                 host name is chosen by Cloudflare"
            );
        }
        let mode = self.tunnel_mode()?;
        if mode.is_none() && self.no_tunnel_setup {
            bail!("--no-tunnel-setup goes with --live-tunnel-name");
        }
        if mode.is_some() {
            if self.server.is_some() {
                bail!(
                    "--live-tunnel publishes the embedded server: it goes with --live-local, \
                     not --live-server"
                );
            }
            if let Some(listen) = self.listen.filter(|l| !l.ip().is_loopback()) {
                bail!(
                    "--live-tunnel keeps the embedded server on loopback; drop --live-listen {listen}"
                );
            }
        }
        if self.local && self.server.is_some() {
            bail!(
                "--live-local runs a server on this machine and --live-server signs in to \
                 another; pick one"
            );
        }
        Ok(())
    }

    /// The tunnel the flags ask for, if any: `--live-tunnel` alone is a
    /// quick tunnel; a name makes it a named one.
    pub fn tunnel_mode(&self) -> Result<Option<crate::tunnel::Mode>> {
        crate::tunnel::Mode::from_flags(
            self.tunnel && self.tunnel_name.is_none(),
            self.tunnel_name.clone(),
            self.hostname.clone(),
        )
    }
}

/// How `h2 play` plays: what the flags and `$H2_LIVE_SERVER` decided.
/// Nothing is contacted to decide it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Offline; `why` is the line to print, if any.
    Offline { why: Option<String> },
    /// `--live-local`: the embedded server.
    Local,
    /// Sign in to `url`; `from` names what chose it (`--live-server`,
    /// `H2_LIVE_SERVER` or `--live`, the default service).
    Server { url: String, from: &'static str },
}

/// What `h2 play` does about Live. `env` is `$H2_LIVE_SERVER`: with
/// `--live` alone it replaces `default` ([`DEFAULT_LIVE_SERVER`] outside
/// tests), and `off`, `none` or `0` there mean offline. `--live-server` and
/// `--live-local` win over it.
pub fn choose(opts: &LiveOpts, env: Option<&str>, default: &str) -> Choice {
    if opts.offline || !opts.enabled {
        return Choice::Offline { why: None };
    }
    if opts.local {
        return Choice::Local;
    }
    if let Some(url) = &opts.server {
        return Choice::Server {
            url: url.trim().trim_end_matches('/').to_string(),
            from: "--live-server",
        };
    }
    match ServerOverride::parse(env) {
        ServerOverride::Off(v) => Choice::Offline {
            why: Some(format!("live: off (H2_LIVE_SERVER={v}); playing offline")),
        },
        ServerOverride::Url(url) => Choice::Server {
            url,
            from: "H2_LIVE_SERVER",
        },
        ServerOverride::Unset => Choice::Server {
            url: default.trim_end_matches('/').to_string(),
            from: "--live",
        },
    }
}

/// A running live session: what to boot the game with, and the embedded
/// server (stopped on drop) if there is one.
pub struct Session {
    pub config: boot::LiveConfig,
    pub account: LocalAccount,
    pub server: String,
    /// The service's API base and id, from its discovery document.
    pub api: String,
    pub server_id: String,
    /// The embedded server's one-use, this-machine-only link signing a
    /// browser in as `account` (docs/live/live-admin.md §4).
    pub signin: Option<String>,
    /// `--tunnel`: cloudflared publishing the embedded server (stopped on
    /// drop), and the lines to print about it.
    tunnel: Option<crate::tunnel::Tunnel>,
    pub tunnel_notices: Vec<String>,
    embedded: Option<live_server::Embedded>,
}

impl Session {
    pub fn store(&self) -> Option<&'static str> {
        self.embedded.as_ref().map(|e| e.store)
    }

    /// The tunnel's public URL when it started (`--tunnel`).
    pub fn public_url(&self) -> Option<&str> {
        self.tunnel.as_ref()?.url.as_deref()
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
    let embedded = match opts.local {
        false => None,
        true => {
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
    let url = match (&embedded, &opts.server) {
        (Some(e), _) => e.url.clone(),
        (None, Some(url)) => url.clone(),
        (None, None) => bail!("no Live server: --live-local or a --live-server URL"),
    };
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
    // The embedded server is the owner's own machine: the account we are
    // about to play as is an admin for this server's lifetime.
    // It may also hand this machine's browser the account: a one-use link
    // carrying the token in its fragment.
    let mut signin = None;
    if let Some(server) = embedded.as_ref() {
        server
            .live
            .grant_admin_runtime(account.xuid, account.gamertag.as_str());
        let entry = crate::live_admin::export_entry(
            &account,
            &client.base,
            &client.discovery.id,
            &account.key_pair()?,
        );
        let token = server
            .live
            .offer_local_signin(entry, live_server::local_signin::OFFER_TTL);
        signin = Some(format!("{}/signin#once={token}", client.base));
    }
    // The game and the sign-in link keep the loopback URL; friends use the
    // tunnel's.
    let (mut tunnel, mut tunnel_notices) = (None, Vec::new());
    if let (Some(mode), Some(server)) = (opts.tunnel_mode()?, embedded.as_ref()) {
        let public = server.live.policy().public.clone();
        let cert = opts
            .origin_cert
            .clone()
            .unwrap_or_else(crate::tunnel::OriginCert::from_env);
        let t = crate::tunnel::open(
            opts.cloudflared.clone(),
            mode.clone(),
            &server.url,
            public,
            "live",
            (!opts.no_tunnel_setup).then_some(&cert),
        )?;
        let policy = server.live.policy();
        tunnel_notices =
            crate::tunnel::notices(t.url.as_deref(), &mode, policy.invite_only, policy.private);
        if let Some(url) = &t.url {
            tunnel_notices.insert(
                1,
                format!("friends join with: h2 play <disc> --live-server {url}"),
            );
        }
        tunnel_notices.extend(live_server::public::unclaimed_admins_notice(
            &server.live.unclaimed_admins(),
        ));
        tunnel = Some(t);
    }
    live_accounts::save(&places.session_accounts, std::slice::from_ref(&account))?;
    Ok(Session {
        config: boot::LiveConfig {
            enabled: true,
            accounts_path: Some(places.session_accounts.clone()),
            server_url: Some(client.base.clone()),
        },
        account,
        api: client.api,
        server_id: client.discovery.id,
        server: client.base,
        signin,
        tunnel,
        tunnel_notices,
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
        Ok(mut d) => {
            // Map pack maps `h2 import-mappack` installed get art too.
            d.add_maps(crate::live_mappack::installed(
                &content::store::default_data_root(),
            ));
            d
        }
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
        assert_eq!(
            parse(&["--live-local"]).unwrap(),
            LiveOpts {
                enabled: true,
                local: true,
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
                && !o.local
                && o.server.as_deref() == Some("https://h.example")
                && o.gamertag.as_deref() == Some("Sneaky Pete")
        );
        let o = parse(&["--live-listen", "127.0.0.1:9000"]).unwrap();
        assert!(o.local, "--live-listen is the embedded server's");
        assert_eq!(o.listen, Some("127.0.0.1:9000".parse().unwrap()));
        assert!(parse(&["--live-local", "--live-listen", "0.0.0.0:8074"]).is_ok());
        assert!(parse(&["--live", "--gamertag", "Pete"]).is_ok());
        assert!(
            parse(&["--gamertag", "Pete"]).is_err(),
            "no Live flag is offline: a gamertag has nothing to sign in to"
        );
        assert!(parse(&["--offline", "--gamertag", "Pete"]).is_err());
        assert!(parse(&["--offline"]).unwrap().offline);
        assert!(parse(&["--no-live"]).unwrap().offline);
        assert!(parse(&["--offline", "--live"]).is_err());
        assert!(parse(&["--offline", "--live-local"]).is_err());
        assert!(parse(&["--no-live", "--live-server", "https://h.example"]).is_err());
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
    fn the_embedded_server_and_another_do_not_mix() {
        let e = parse(&["--live-local", "--live-server", "https://h.example"]).unwrap_err();
        assert!(format!("{e}").contains("pick one"), "{e}");
        let e = parse(&["--live-server", "https://h.example", "--live-local"]).unwrap_err();
        assert!(format!("{e}").contains("pick one"), "{e}");
        // --live with --live-local is just the embedded server.
        let o = parse(&["--live", "--live-local"]).unwrap();
        assert_eq!(choose(&o, None, DEFAULT_LIVE_SERVER), Choice::Local);
    }

    /// Every flag combination, and what it plays on: nothing is contacted
    /// to decide (a URL nothing listens on would do as the default).
    #[test]
    fn the_flag_matrix() {
        let server = |url: &str, from| Choice::Server {
            url: url.into(),
            from,
        };
        let offline = Choice::Offline { why: None };
        let default = "https://default.example";
        let pick = |args: &[&str], env: Option<&str>| choose(&parse(args).unwrap(), env, default);
        // The CLI is offline unless asked, whatever $H2_LIVE_SERVER says.
        assert_eq!(pick(&[], None), offline);
        assert_eq!(pick(&[], Some("https://env.example")), offline);
        assert_eq!(pick(&["--offline"], None), offline);
        assert_eq!(pick(&["--offline"], Some("https://env.example")), offline);
        // --live: the default service, or the variable's.
        assert_eq!(pick(&["--live"], None), server(default, "--live"));
        assert_eq!(
            pick(&["--live"], Some("https://env.example/")),
            server("https://env.example", "H2_LIVE_SERVER")
        );
        assert_eq!(pick(&["--live"], Some(" ")), server(default, "--live"));
        for off in ["off", "none", "0", "OFF"] {
            let Choice::Offline { why: Some(why) } = pick(&["--live"], Some(off)) else {
                panic!("{off}")
            };
            assert!(why.contains("H2_LIVE_SERVER"), "{why}");
        }
        // --live-server: that one; the flag wins over the variable.
        for env in [None, Some("https://env.example"), Some("off")] {
            assert_eq!(
                pick(&["--live-server", "https://h.example/"], env),
                server("https://h.example", "--live-server")
            );
            assert_eq!(
                pick(&["--live", "--live-server", "https://h.example"], env),
                server("https://h.example", "--live-server")
            );
        }
        // --live-local and what implies it: the embedded server.
        for args in [
            &["--live-local"][..],
            &["--live-listen", "127.0.0.1:9000"],
            &["--live-tunnel"],
            &["--tunnel"],
            &["--live-tunnel-name", "h2"],
            &["--live-local", "--live-tunnel"],
        ] {
            assert_eq!(pick(args, Some("off")), Choice::Local, "{args:?}");
        }
        // The real default is the shared constant.
        assert_eq!(DEFAULT_LIVE_SERVER, "https://live.forbes.red");
        assert_eq!(
            choose(&parse(&["--live"]).unwrap(), None, DEFAULT_LIVE_SERVER),
            server("https://live.forbes.red", "--live")
        );
    }

    #[test]
    fn an_embedded_session_makes_then_reuses_an_account() {
        let dir = temp("session");
        let places = Places::under(&dir);
        let opts = LiveOpts {
            enabled: true,
            local: true,
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

    #[test]
    fn an_embedded_session_offers_a_one_use_signin_link() {
        let dir = temp("signin");
        let places = Places::under(&dir);
        let opts = LiveOpts {
            enabled: true,
            local: true,
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
        let link = session
            .signin
            .clone()
            .expect("an embedded server offers a link");
        let token = link
            .strip_prefix(&format!("{}/signin#once=", session.server))
            .expect("the token rides in the fragment");
        assert_eq!(token.len(), 44);
        #[allow(clippy::result_large_err)] // ureq's own error type
        let redeem = || {
            ureq::post(&format!("{}/v1/local/signin", session.server))
                .set("content-type", "application/json")
                .send_string(&serde_json::json!({ "once": token }).to_string())
        };
        let entry: serde_json::Value = redeem().unwrap().into_json().unwrap();
        let client = Client::at(&session.server).unwrap();
        let key = session.account.key_pair().unwrap();
        assert_eq!(
            entry,
            crate::live_admin::export_entry(
                &session.account,
                &client.base,
                &client.discovery.id,
                &key
            )
        );
        assert_eq!(entry["gamertag"], "test user");
        assert_eq!(entry["server"], session.server.as_str());
        assert_eq!(entry["key"], key.secret_b64());
        match redeem() {
            Err(ureq::Error::Status(404, _)) => {}
            other => panic!("a second use: {other:?}"),
        }
        drop(session);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tunnel_flags() {
        let o = parse(&["--live-local", "--tunnel"]).unwrap();
        assert_eq!(o.tunnel_mode().unwrap(), Some(crate::tunnel::Mode::Quick));
        let o = parse(&[
            "--live-local",
            "--tunnel-name",
            "h2",
            "--hostname",
            "live.example.net",
        ])
        .unwrap();
        assert_eq!(
            o.tunnel_mode().unwrap().unwrap().fixed_url().as_deref(),
            Some("https://live.example.net")
        );
        assert!(parse(&["--tunnel"]).unwrap().local, "implies --live-local");
        assert!(
            parse(&["--live-server", "https://h.example", "--tunnel"]).is_err(),
            "only the embedded server is tunnelled"
        );
        assert!(parse(&["--live-local", "--live-listen", "0.0.0.0:9000", "--tunnel"]).is_err());
        // --tunnel is --live-tunnel's alias, so a name makes it a named one.
        assert_eq!(
            parse(&["--live-local", "--tunnel", "--tunnel-name", "h2"])
                .unwrap()
                .tunnel_mode()
                .unwrap(),
            Some(crate::tunnel::Mode::Named {
                name: "h2".into(),
                hostname: None
            })
        );
        assert!(parse(&["--live-local", "--hostname", "a.example"]).is_err());
    }

    #[test]
    fn live_tunnel_flags() {
        // --live-tunnel is --live-local too.
        let o = parse(&["--live-tunnel"]).unwrap();
        assert!(o.enabled && o.local && o.tunnel && !o.no_tunnel_setup);
        assert_eq!(o.tunnel_mode().unwrap(), Some(crate::tunnel::Mode::Quick));

        let named = crate::tunnel::Mode::Named {
            name: "h2play".into(),
            hostname: Some("play.forbes.red".into()),
        };
        let o = parse(&[
            "--live-tunnel",
            "--live-tunnel-name",
            "h2play",
            "--live-tunnel-url",
            "https://Play.Forbes.red/",
        ])
        .unwrap();
        assert_eq!(o.tunnel_mode().unwrap(), Some(named.clone()));
        assert_eq!(
            named.fixed_url().as_deref(),
            Some("https://play.forbes.red")
        );
        // The old names say the same.
        let old = parse(&[
            "--live-local",
            "--tunnel",
            "--tunnel-name",
            "h2play",
            "--hostname",
            "play.forbes.red",
        ])
        .unwrap();
        assert_eq!(old.tunnel_mode().unwrap(), Some(named.clone()));
        let mixed = parse(&[
            "--live-tunnel",
            "--tunnel-name",
            "h2play",
            "--live-tunnel-url",
            "https://play.forbes.red",
            "--no-tunnel-setup",
        ])
        .unwrap();
        assert_eq!(mixed.tunnel_mode().unwrap(), Some(named));
        assert!(mixed.no_tunnel_setup);
        // A name without a URL: runs, the URL unknown.
        assert_eq!(
            parse(&["--live-tunnel", "--live-tunnel-name", "h2play"])
                .unwrap()
                .tunnel_mode()
                .unwrap(),
            Some(crate::tunnel::Mode::Named {
                name: "h2play".into(),
                hostname: None
            })
        );

        // The URL: https, a host, nothing more.
        for bad in [
            "http://play.forbes.red",
            "play.forbes.red",
            "https://play.forbes.red:8443",
            "https://play.forbes.red/live",
            "https://play.forbes.red?x",
        ] {
            let e = parse(&[
                "--live-tunnel",
                "--live-tunnel-name",
                "h2play",
                "--live-tunnel-url",
                bad,
            ])
            .unwrap_err();
            assert!(format!("{e}").contains("https://HOST"), "{bad}: {e}");
        }
        assert!(
            parse(&[
                "--live-tunnel",
                "--live-tunnel-url",
                "https://play.forbes.red"
            ])
            .is_err(),
            "a quick tunnel has no URL of ours"
        );
        assert!(
            parse(&["--live-tunnel", "--live-server", "https://h.example"]).is_err(),
            "only the embedded server is tunnelled"
        );
        assert!(
            parse(&["--live-tunnel-name", "h2play"]).unwrap().local,
            "a tunnel name implies --live-local"
        );
        assert!(
            parse(&["--live-local", "--no-tunnel-setup"]).is_err(),
            "no tunnel to set up"
        );
        assert!(parse(&["--live-tunnel-url"]).is_err(), "needs a value");
    }

    /// A test-only cloudflared: prints a quick tunnel URL, records its
    /// arguments and pid, then waits to be stopped.
    fn fake_cloudflared(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let p = bin.join("cloudflared");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\necho \"$@\" > {d}/argv\necho $$ > {d}/pid\n\
                 echo '2025-03-04T10:15:03Z INF |  https://friendly-test-host.trycloudflare.com  |' >&2\n\
                 exec sleep 30\n",
                d = dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn a_tunnelled_session_publishes_the_url_and_keeps_signin_local() {
        let dir = temp("tunnel");
        let places = Places::under(&dir);
        let opts = LiveOpts {
            enabled: true,
            local: true,
            tunnel: true,
            cloudflared: Some(fake_cloudflared(&dir)),
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
        let public = "https://friendly-test-host.trycloudflare.com";
        assert_eq!(session.public_url(), Some(public));
        // cloudflared forwards to the embedded server on loopback.
        let argv = std::fs::read_to_string(dir.join("argv")).unwrap();
        assert_eq!(
            argv.trim(),
            format!(
                "tunnel --no-autoupdate --grace-period 1s --url {}",
                session.server
            )
        );
        assert!(session.server.starts_with("http://127.0.0.1:"));
        // Discovery names the public URL; the game and the link stay local.
        let client = Client::at(&session.server).unwrap();
        assert_eq!(client.discovery.url.as_deref(), Some(public));
        assert_eq!(client.discovery.api, "/v1");
        assert_eq!(
            session.config.server_url.as_deref(),
            Some(session.server.as_str())
        );
        let link = session.signin.clone().unwrap();
        assert!(link.starts_with(&format!("{}/signin#once=", session.server)));
        assert!(
            session
                .tunnel_notices
                .iter()
                .any(|l| l.contains(&format!("--live-server {public}")))
        );
        // What cloudflared delivers (from loopback, with its headers) does
        // not get the link's entry.
        let token = link.rsplit_once("#once=").unwrap().1.to_string();
        let tunnelled = ureq::post(&format!("{}/v1/local/signin", session.server))
            .set("content-type", "application/json")
            .set("cf-connecting-ip", "203.0.113.9")
            .set("x-forwarded-for", "203.0.113.9")
            .send_string(&serde_json::json!({ "once": token }).to_string());
        match tunnelled {
            Err(ureq::Error::Status(404, _)) => {}
            other => panic!("a tunnelled redeem: {other:?}"),
        }
        // Stopping the session stops cloudflared.
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        drop(session);
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "cloudflared outlived the session");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_named_tunnel_is_set_up_before_the_session_publishes_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("named-setup");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("cloudflared");
        let certs = dir.join("cloudflared-home");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" >> {d}/calls\n\
                 case \"$1 $2\" in\n\
                 'tunnel login') mkdir -p {c}; echo cert > {c}/cert.pem ;;\n\
                 'tunnel list') echo '[]' ;;\n\
                 'tunnel create') ;;\n\
                 'tunnel route') echo 'INF Added CNAME play.example.net which will route to this tunnel' >&2 ;;\n\
                 *) echo 'INF Registered tunnel connection connIndex=0' >&2; exec sleep 30 ;;\n\
                 esac\n",
                d = dir.display(),
                c = certs.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut opts = parse(&[
            "--live-tunnel",
            "--live-tunnel-name",
            "h2play",
            "--live-tunnel-url",
            "https://play.example.net",
        ])
        .unwrap();
        opts.cloudflared = Some(fake);
        opts.origin_cert = Some(crate::tunnel::OriginCert {
            explicit: None,
            dirs: vec![certs.clone()],
        });
        let calls = || -> Vec<String> {
            std::fs::read_to_string(dir.join("calls"))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        };
        let session = start(
            &opts,
            live_server::MEMORY_DB,
            &Places::under(&dir),
            Default::default(),
            "test.user",
        )
        .unwrap();
        assert_eq!(session.public_url(), Some("https://play.example.net"));
        let run = format!(
            "tunnel --no-autoupdate --grace-period 1s run --url {} h2play",
            session.server
        );
        drop(session);
        assert_eq!(
            calls(),
            [
                "tunnel login",
                "tunnel list --output json",
                "tunnel create h2play",
                "tunnel route dns h2play play.example.net",
                run.as_str()
            ]
        );

        // --no-tunnel-setup: only the run.
        std::fs::remove_file(dir.join("calls")).unwrap();
        opts.no_tunnel_setup = true;
        let session = start(
            &opts,
            live_server::MEMORY_DB,
            &Places::under(&dir),
            Default::default(),
            "test.user",
        )
        .unwrap();
        let run = format!(
            "tunnel --no-autoupdate --grace-period 1s run --url {} h2play",
            session.server
        );
        drop(session);
        assert_eq!(calls(), [run]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn another_server_gets_no_link() {
        // `--live-server`: the account is that server's business; there is
        // no embedded server to offer a link.
        let dir = temp("signin-remote");
        let places = Places::under(&dir);
        let embedded = start(
            &LiveOpts {
                enabled: true,
                local: true,
                ..LiveOpts::default()
            },
            live_server::MEMORY_DB,
            &Places::under(&dir.join("host")),
            Default::default(),
            "host",
        )
        .unwrap();
        let remote = start(
            &LiveOpts {
                enabled: true,
                server: Some(embedded.server.clone()),
                ..LiveOpts::default()
            },
            live_server::MEMORY_DB,
            &places,
            Default::default(),
            "guest",
        )
        .unwrap();
        assert!(remote.signin.is_none());
        drop(remote);
        drop(embedded);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
