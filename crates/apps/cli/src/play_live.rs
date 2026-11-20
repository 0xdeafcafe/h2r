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
use live_proto::{Gamertag, JoinLink, ServerOverride, Xuid};

use crate::live_accounts::{self, LocalAccount};
use crate::live_cmd::{Client, status_of};
use live_proto::sign::KeyPair;

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
    /// A validated session link. It enables Live but never selects the server.
    pub join: Option<JoinLink>,
    /// `--live-listen`: where the embedded server binds (default
    /// 127.0.0.1 on a free port).
    pub listen: Option<SocketAddr>,
    /// `--gamertag`: the account to play as (made if missing).
    pub gamertag: Option<String>,
    /// `--invite CODE`: for making the account on an invite-only server.
    pub invite: Option<String>,
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
    /// `--signin-terminal`: a code (or invite) the account needs is asked
    /// for on the terminal before the game starts, as before; without it
    /// the game asks for the code when the player signs in
    /// (docs/live/live.md, "In-game sign-in").
    pub signin_terminal: bool,
}

impl LiveOpts {
    /// Takes `flag` (and its value) if it is one of ours.
    pub fn take(&mut self, flag: &str, value: &mut dyn FnMut() -> Result<String>) -> Result<bool> {
        match flag {
            "--live" => self.enabled = true,
            "--join" => {
                if self.join.is_some() {
                    bail!("Only one --join URL can be opened at a time.");
                }
                self.join = Some(value()?.parse().context("Invalid --join URL")?);
                self.enabled = true;
            }
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
            "--invite" => self.invite = Some(value()?),
            "--signin-terminal" => self.signin_terminal = true,
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
        if self.offline && self.join.is_some() {
            bail!("Joining a session requires Live. Remove --offline or --no-live.");
        }
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
        if self.invite.is_some() && !self.enabled {
            bail!(
                "--invite makes the Live account to play as; it goes with --live or --live-server"
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

    /// Resolve the service before account discovery or creation. A URL from
    /// another service can only be used after explicitly choosing its server.
    pub fn play_choice(&self, env: Option<&str>) -> Result<Choice> {
        let choice = choose(self, env, DEFAULT_LIVE_SERVER);
        match &choice {
            Choice::Offline { .. } if self.join.is_some() => {
                bail!("Joining a session requires Live. Choose a Live server to sign in.");
            }
            Choice::Server { url, .. } => self.check_join_server(url)?,
            _ => {}
        }
        Ok(choice)
    }

    pub fn check_join_server(&self, url: &str) -> Result<()> {
        if let Some(link) = &self.join
            && !link.matches_server(url)
        {
            bail!(
                "This game is on {}. To join it, choose that service with --live-server {}.",
                link.server(),
                link.server()
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
    /// The account the game signs in as; `None` while it waits for the
    /// code the game asks for (`waiting`).
    pub account: Option<LocalAccount>,
    /// The gamertag handed to the game as needing a code.
    pub waiting: Option<String>,
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
    let mut terminal = terminal_ask;
    // The terminal asks only when told to; otherwise a code is the game's
    // to ask for (docs/live/live.md, "In-game sign-in").
    let ask: Ask = if opts.signin_terminal && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        Some(&mut terminal)
    } else {
        None
    };
    let (account, created) = match bootstrap(
        &client,
        &places.accounts,
        opts.gamertag.as_deref(),
        default_name,
        opts.invite.as_deref(),
        embedded.is_some(),
        ask,
    ) {
        Ok(found) => found,
        Err(e) if !opts.signin_terminal && e.downcast_ref::<NeedsCode>().is_some() => {
            let need = e.downcast::<NeedsCode>().expect("checked");
            eprintln!(
                "live: {} is on {} already and this machine has no key for it; the game asks for its code",
                need.gamertag, client.discovery.id
            );
            write_waiting(&places.session_accounts, &need, &client.base)?;
            return Ok(Session {
                config: boot::LiveConfig {
                    enabled: true,
                    accounts_path: Some(places.session_accounts.clone()),
                    server_url: Some(client.base.clone()),
                    offer_new_account: false,
                },
                account: None,
                waiting: Some(need.gamertag),
                api: client.api,
                server_id: client.discovery.id,
                server: client.base,
                signin: None,
                tunnel: None,
                tunnel_notices: Vec::new(),
                embedded,
            });
        }
        Err(e) => return Err(e),
    };
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
            offer_new_account: false,
        },
        account: Some(account),
        waiting: None,
        api: client.api,
        server_id: client.discovery.id,
        server: client.base,
        signin,
        tunnel,
        tunnel_notices,
        embedded,
    })
}

/// The account is on the server already and this machine has no key for
/// it, with no terminal to ask for the code: the game asks instead
/// ([`write_waiting`]). Its text is the command that recovers it by hand.
#[derive(Debug, Clone)]
pub struct NeedsCode {
    pub gamertag: String,
    /// The account's xuid on the server, when it said.
    pub xuid: Option<Xuid>,
    message: String,
}

impl std::fmt::Display for NeedsCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NeedsCode {}

/// Hands the game `need` as its one account, marked `needs_code`: picking
/// it at the game's Xbox Live sign-in asks for the code. No key is in it.
pub fn write_waiting(path: &Path, need: &NeedsCode, server: &str) -> Result<()> {
    let xuid = need.xuid.unwrap_or_else(|| stand_in_xuid(&need.gamertag));
    let entry = serde_json::json!([{
        "xuid": xuid,
        "gamertag": need.gamertag,
        "server": server,
        "needs_code": true,
    }]);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{entry:#}\n"))
        .with_context(|| format!("writing {}", path.display()))
}

/// A stable nonzero xuid for a gamertag the server gave none for: the game
/// lists the identity by it until the real one replaces it.
fn stand_in_xuid(gamertag: &str) -> Xuid {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in gamertag.to_ascii_lowercase().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    Xuid(0x0009_0000_0000_0000 | (h & 0xffff_ffff) | 1)
}

/// Asks the player a question on the terminal; `None` when stdin is not one.
pub type Ask<'a> = Option<&'a mut dyn FnMut(&str) -> Result<String>>;

/// The terminal's [`Ask`]: the question on stderr, the answer from stdin.
pub fn terminal_ask(question: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    eprint!("{question} ");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Seconds since the epoch an account was last picked to play
/// (`last_played` in accounts.json); 0 if never.
fn last_played(a: &LocalAccount) -> i64 {
    a.extra
        .get("last_played")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

/// A gamertag as a shell word.
fn shell_word(s: &str) -> String {
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The account to play as on `client`'s server (docs/live/live.md
/// "Playing on Live"):
///
/// 1. a local one there (named `want`, if given) whose key the server
///    still lists as current;
/// 2. else the player's identity from another server: `want`, else the
///    gamertag of the most recently played local account elsewhere, else
///    one from `default_name` (the OS user name), keeping that account's
///    key when there is one:
///    - not on this server yet: created with that key (with `invite`, or
///      one asked for, when the server is invite-only);
///    - already there (made from another device or the web): recovered
///      with a code the player types, adding this machine's key;
///    - on the embedded server (`local`) a taken default name is numbered
///      instead, as before.
///
/// `ask` is how to ask the player (None: not a terminal, so fail with the
/// command to run). Returns the account and whether it was added now.
pub fn bootstrap(
    client: &Client,
    accounts_path: &Path,
    want: Option<&str>,
    default_name: &str,
    invite: Option<&str>,
    local: bool,
    mut ask: Ask,
) -> Result<(LocalAccount, bool)> {
    let mut accounts = live_accounts::load(accounts_path)?;
    let here = |a: &LocalAccount| a.is_on(&client.base, &client.discovery.id);
    let named = |a: &LocalAccount| want.is_none_or(|w| a.gamertag.eq_ignore_ascii_case(w));
    let mut found = None;
    for a in accounts.iter().filter(|a| here(a) && named(a)) {
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
            found = Some(a.clone());
            break;
        }
    }
    if let Some(account) = found {
        return Ok((keep(accounts_path, &mut accounts, account)?, false));
    }

    // The player's identity on other servers, most recently played last.
    let mut elsewhere: Vec<(usize, &LocalAccount)> = accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| !here(a) && named(a) && a.key_pair().is_ok())
        .collect();
    elsewhere.sort_by_key(|(i, a)| (last_played(a), *i));
    let prior = elsewhere.last().map(|(_, a)| (*a).clone());
    let base = want
        .map(str::to_string)
        .or_else(|| prior.as_ref().map(|a| a.gamertag.clone()))
        .unwrap_or_else(|| live_accounts::gamertag_from(default_name));
    let reuse = prior
        .filter(|a| a.gamertag.eq_ignore_ascii_case(&base))
        .and_then(|a| a.key_pair().ok());
    let tag = Gamertag::new(&base).with_context(|| format!("{base:?} is not a valid gamertag"))?;

    if local {
        return numbered(
            client,
            accounts_path,
            &mut accounts,
            &base,
            want.is_none(),
            reuse,
        );
    }

    let (exists, there) = match client.get::<serde_json::Value>(
        &format!("/accounts/{}", crate::live_query::path_part(tag.as_str())),
        &[],
    ) {
        Ok(v) => (
            true,
            v.get("xuid")
                .and_then(serde_json::Value::as_str)
                .and_then(|x| Xuid::parse(x).ok()),
        ),
        Err(e) if status_of(&e) == Some(404) => (false, None),
        Err(e) => return Err(e),
    };
    let host = &client.discovery.id;
    let account = if exists {
        let Some(ask) = ask.as_mut() else {
            return Err(NeedsCode {
                gamertag: tag.as_str().to_string(),
                xuid: there,
                message: format!(
                    "{tag} is already an account on {host} and this machine has no key for it. \
                     Recover it here with a code from Account on {}/dash or your authenticator app:\n  \
                     h2 live account recover {} --server {}",
                    client.base,
                    shell_word(tag.as_str()),
                    client.base
                ),
            }
            .into());
        };
        let mut key = reuse.unwrap_or_else(KeyPair::generate);
        let mut tries = 0;
        loop {
            tries += 1;
            let code = ask(&format!(
                "{tag} is on {host} already; enter the code from Account on {}/dash or your authenticator:",
                client.base
            ))?;
            let key_now = KeyPair::from_secret_b64(&key.secret_b64()).expect("our own key");
            match crate::live_recovery::recover_with_key(
                client,
                accounts_path,
                &tag,
                &code,
                key_now,
            ) {
                Ok(a) => break a,
                // The key is taken (by another account): a fresh one.
                Err(e) if status_of(&e) == Some(409) => key = KeyPair::generate(),
                Err(e) if status_of(&e) == Some(429) || tries >= 3 => return Err(e),
                // A wrong or mistyped code: ask again, same key.
                Err(e) => eprintln!("live: {e:#}"),
            }
            accounts = live_accounts::load(accounts_path)?;
        }
    } else {
        let invite_only = client
            .get::<serde_json::Value>("/server", &[])
            .ok()
            .and_then(|v| v["invite_only"].as_bool())
            .unwrap_or(false);
        let mut code = invite.map(str::to_string);
        if invite_only && code.is_none() {
            let Some(ask) = ask.as_mut() else {
                bail!(
                    "{host} needs an invite code to make {tag} there: pass --invite CODE (an \
                     admin makes one on the Server screen at {}/dash). If {tag} is yours \
                     on {host} already, recover it: h2 live account recover {} --server {}",
                    client.base,
                    shell_word(tag.as_str()),
                    client.base
                );
            };
            code = Some(ask(&format!(
                "{host} needs an invite to make {tag} there; enter an invite code:"
            ))?);
        }
        let key = reuse.unwrap_or_else(KeyPair::generate);
        let mut tries = 0;
        loop {
            tries += 1;
            let key_now = KeyPair::from_secret_b64(&key.secret_b64()).expect("our own key");
            match client.create_account_with(key_now, tag.as_str(), code.clone()) {
                Ok((created, key)) => {
                    accounts.retain(|a| !(here(a) && a.gamertag.eq_ignore_ascii_case(&base)));
                    break LocalAccount::new(
                        created.xuid,
                        created.gamertag.as_str(),
                        &client.base,
                        &client.discovery,
                        &key,
                    );
                }
                Err(e) if status_of(&e) == Some(403) && tries < 3 && ask.is_some() => {
                    eprintln!("live: {e:#}");
                    let ask = ask.as_mut().expect("checked");
                    code = Some(ask("enter an invite code:")?);
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("creating {tag} on {host}"));
                }
            }
        }
    };
    Ok((keep(accounts_path, &mut accounts, account)?, true))
}

/// Stores `account` (replacing the same xuid) stamped as just played.
fn keep(
    path: &Path,
    accounts: &mut Vec<LocalAccount>,
    mut account: LocalAccount,
) -> Result<LocalAccount> {
    account
        .extra
        .insert("last_played".into(), crate::live_cmd::now().into());
    live_accounts::upsert(accounts, account.clone());
    live_accounts::save(path, accounts)?;
    Ok(account)
}

/// A server on this machine: `base`, numbered while taken if `number`.
fn numbered(
    client: &Client,
    accounts_path: &Path,
    accounts: &mut Vec<LocalAccount>,
    base: &str,
    number: bool,
    mut reuse: Option<KeyPair>,
) -> Result<(LocalAccount, bool)> {
    for n in 1..=20 {
        let tag = if n == 1 {
            base.to_string()
        } else {
            live_accounts::numbered(base, n)
        };
        let key = reuse.take().unwrap_or_else(KeyPair::generate);
        match client.create_account_with(key, &tag, None) {
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
                return Ok((keep(accounts_path, accounts, account)?, true));
            }
            Err(e) if status_of(&e) == Some(409) && number => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("creating {tag:?} on {}", client.base));
            }
        }
    }
    bail!(
        "every gamertag from {base:?} to {:?} is taken; pass --gamertag",
        live_accounts::numbered(base, 20)
    )
}

/// The game art in `dir`, extracted from `disc` first unless it already
/// holds this disc's art. Any failure is a warning: the web app keeps its
/// own art.
pub fn game_art(disc: &Path, dir: &Path) -> Option<live_server::art::GameArt> {
    let data = content::store::default_data_root();
    let mut disc = match crate::live_art::Disc::scan(disc) {
        Ok(mut d) => {
            // Map pack maps `h2 import-mappack` installed get art too.
            d.add_maps(crate::live_mappack::installed(&data));
            // And the Halo 1 maps the import converted there.
            d.add_halo1(crate::live_halo1::installed(&data));
            // And the installed mods, keyed by their MapIds (resolved
            // only when the art is extracted).
            d.key_mods(&crate::live_mods::package_ids(&data));
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
        let scratch = dir.with_extension("mods");
        let _ = std::fs::remove_dir_all(&scratch);
        for n in disc.add_installed_mods(&data, &scratch) {
            eprintln!("live: {n}");
        }
        let done = crate::live_art::extract(&disc, dir);
        let _ = std::fs::remove_dir_all(&scratch);
        if let Err(e) = done {
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

    /// An embedded server in `dir` with `policy`, and a client for it.
    fn server_at(
        dir: &Path,
        name: &str,
        policy: live_server::Policy,
    ) -> (live_server::Embedded, Client) {
        // Each its own server id, as two real servers would have.
        live_server::ServerIdentity::open(&dir.join(name), Some(&format!("{name}.test"))).unwrap();
        let server = live_server::start_embedded_with(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir.join(name),
            Default::default(),
            policy,
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        (server, client)
    }

    fn kid_of(a: &LocalAccount) -> String {
        a.key_pair().unwrap().kid().to_string()
    }

    #[test]
    fn moving_servers_keeps_the_gamertag_and_key() {
        let dir = temp("move");
        let accounts = dir.join("accounts.json");
        let (_a, old) = server_at(&dir, "a", Default::default());
        let (_b, new) = server_at(&dir, "b", Default::default());
        let (afr, _) = bootstrap(&old, &accounts, Some("afr"), "x", None, true, None).unwrap();
        // Not on b yet: made there with the same key, not the OS name.
        let (moved, created) =
            bootstrap(&new, &accounts, None, "os.user", None, false, None).unwrap();
        assert!(created);
        assert_eq!(moved.gamertag, "afr");
        assert_eq!(kid_of(&moved), kid_of(&afr));
        assert_eq!(live_accounts::load(&accounts).unwrap().len(), 2);
        // Next time it is simply found.
        let (again, created) =
            bootstrap(&new, &accounts, None, "os.user", None, false, None).unwrap();
        assert!(!created);
        assert_eq!(again.xuid, moved.xuid);
    }

    #[test]
    fn the_most_recently_played_identity_is_the_one_moved() {
        let dir = temp("recent");
        let accounts = dir.join("accounts.json");
        let (_a, a) = server_at(&dir, "a", Default::default());
        let (_b, b) = server_at(&dir, "b", Default::default());
        let (_c, c) = server_at(&dir, "c", Default::default());
        bootstrap(&a, &accounts, Some("Older"), "x", None, true, None).unwrap();
        bootstrap(&b, &accounts, Some("Newer"), "x", None, true, None).unwrap();
        // Play as Older again: it is now the latest.
        let mut all = live_accounts::load(&accounts).unwrap();
        for e in all.iter_mut() {
            let t = if e.gamertag == "Older" { 200 } else { 100 };
            e.extra.insert("last_played".into(), t.into());
        }
        live_accounts::save(&accounts, &all).unwrap();
        let (moved, _) = bootstrap(&c, &accounts, None, "x", None, false, None).unwrap();
        assert_eq!(moved.gamertag, "Older");
    }

    #[test]
    fn a_gamertag_already_there_is_recovered_with_a_code() {
        let dir = temp("recover");
        let accounts = dir.join("accounts.json");
        let (_a, old) = server_at(&dir, "a", Default::default());
        let (_b, new) = server_at(&dir, "b", Default::default());
        let (afr, _) = bootstrap(&old, &accounts, Some("afr"), "x", None, true, None).unwrap();
        // afr signed up on b from the web: a key this machine lacks.
        let (web, _) = new.create_account("afr", None).unwrap();
        let secret =
            live_proto::recovery::TotpSecret::from_base32(&web.recovery.expect("recovery").secret)
                .unwrap();

        // Not a terminal: the command to run, as a NeedsCode the game can
        // take over.
        let err = bootstrap(&new, &accounts, None, "x", None, false, None).unwrap_err();
        let need = err.downcast_ref::<NeedsCode>().expect("a code is needed");
        assert_eq!((need.gamertag.as_str(), need.xuid), ("afr", Some(web.xuid)));
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&format!(
                "h2 live account recover afr --server {}",
                new.base
            )),
            "{msg}"
        );

        // A terminal: a wrong code, then the right one.
        let mut asked = Vec::new();
        let mut answers = vec![
            format!(
                "{:06}",
                (secret.code_at(crate::live_cmd::now()) + 1) % 1_000_000
            ),
            live_proto::recovery::format_code(secret.code_at(crate::live_cmd::now())),
        ]
        .into_iter();
        let mut ask = |q: &str| {
            asked.push(q.to_string());
            Ok(answers.next().unwrap())
        };
        let (got, added) =
            bootstrap(&new, &accounts, None, "x", None, false, Some(&mut ask)).unwrap();
        assert!(added);
        assert_eq!((got.xuid, got.gamertag.as_str()), (web.xuid, "afr"));
        assert_eq!(kid_of(&got), kid_of(&afr), "one key for the player");
        assert!(
            asked[0].contains("afr is on") && asked[0].contains("/dash"),
            "{asked:?}"
        );
        let keys = new.keys(web.xuid).unwrap();
        assert!(keys.keys.iter().any(|k| k.kid == kid_of(&afr)));
        // Found from now on.
        let (again, added) = bootstrap(&new, &accounts, None, "x", None, false, None).unwrap();
        assert!(!added);
        assert_eq!(again.xuid, web.xuid);
    }

    /// `h2 play --live` with an account the server has and this machine
    /// has no key for: no terminal prompt, the game is handed the identity
    /// marked needs_code (and asks for the code itself). With
    /// `--signin-terminal` and no terminal it stops with the command.
    #[test]
    fn a_code_the_account_needs_is_left_to_the_game() {
        let dir = temp("handoff");
        let (_b, new) = server_at(&dir, "b", Default::default());
        let (web, _) = new.create_account("afr", None).unwrap();
        let opts = parse(&["--live-server", &new.base, "--gamertag", "afr"]).unwrap();
        let places = Places::under(&dir);
        let session = start(
            &opts,
            live_server::MEMORY_DB,
            &places,
            Default::default(),
            "x",
        )
        .unwrap();
        assert!(session.account.is_none());
        assert_eq!(session.waiting.as_deref(), Some("afr"));
        assert_eq!(
            session.config.accounts_path.as_ref(),
            Some(&places.session_accounts)
        );
        assert_eq!(
            session.config.server_url.as_deref(),
            Some(new.base.as_str())
        );
        let handed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&places.session_accounts).unwrap()).unwrap();
        let handed = handed.as_array().unwrap();
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0]["gamertag"], "afr");
        assert_eq!(handed[0]["needs_code"], true);
        assert_eq!(handed[0]["xuid"], serde_json::to_value(web.xuid).unwrap());
        assert!(handed[0].get("key").is_none(), "no key handed");
        drop(session);

        let terminal = parse(&[
            "--live-server",
            &new.base,
            "--gamertag",
            "afr",
            "--signin-terminal",
        ])
        .unwrap();
        let err = match start(
            &terminal,
            live_server::MEMORY_DB,
            &places,
            Default::default(),
            "x",
        ) {
            Err(e) => format!("{e:#}"),
            Ok(_) => panic!("no terminal to ask on"),
        };
        assert!(err.contains("h2 live account recover afr"), "{err}");
        assert!(
            parse(&["--live", "--signin-terminal"])
                .unwrap()
                .signin_terminal
        );
    }

    #[test]
    fn a_stand_in_xuid_is_stable_and_nonzero() {
        assert_eq!(stand_in_xuid("Pete"), stand_in_xuid("pete"));
        assert_ne!(stand_in_xuid("Pete"), stand_in_xuid("Bob"));
        assert_ne!(stand_in_xuid("Pete").0, 0);
    }

    #[test]
    fn an_invite_only_server_takes_an_invite_from_the_flag_or_the_terminal() {
        let dir = temp("invite");
        let accounts = dir.join("accounts.json");
        let (_a, old) = server_at(&dir, "a", Default::default());
        let policy = || live_server::Policy {
            invite_only: true,
            ..Default::default()
        };
        let (_b, new) = server_at(&dir, "b", policy());
        bootstrap(&old, &accounts, Some("afr"), "x", None, true, None).unwrap();
        let op = live_server::ServerIdentity::load(&dir.join("b")).unwrap();
        let invite =
            || live_proto::sign::Invite::issue(&op.key, &op.id, crate::live_cmd::now() + 3600);

        let err = bootstrap(&new, &accounts, None, "x", None, false, None).unwrap_err();
        assert!(format!("{err:#}").contains("--invite CODE"), "{err:#}");
        let (got, _) = bootstrap(&new, &accounts, None, "x", Some(&invite()), false, None).unwrap();
        assert_eq!(got.gamertag, "afr");

        // Another server, the invite typed at the terminal.
        let (_c, third) = server_at(&dir, "c", policy());
        let op = live_server::ServerIdentity::load(&dir.join("c")).unwrap();
        let code = live_proto::sign::Invite::issue(&op.key, &op.id, crate::live_cmd::now() + 3600);
        let mut ask = |_: &str| Ok(code.clone());
        let (got, _) =
            bootstrap(&third, &accounts, None, "x", None, false, Some(&mut ask)).unwrap();
        assert_eq!(got.gamertag, "afr");
    }

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
    fn join_link_enables_live_and_preserves_the_players_server_choice() {
        let link = "h2recomp://join?server=https%3A%2F%2Fother.example&session=8000000000001234";
        let opts = parse(&["--join", link]).unwrap();
        assert!(opts.join.as_ref().unwrap().matches_server("https://other.example"));
        assert!(opts.play_choice(None).unwrap_err().to_string().contains("--live-server"));
        assert!(opts.play_choice(Some("off")).is_err());
        assert!(opts.play_choice(Some("https://other.example")).is_ok());
        let pick = |args: &[&str]| choose(&parse(args).unwrap(), None, DEFAULT_LIVE_SERVER);
        assert_eq!(pick(&["--join", link]), pick(&["--live"]));
        assert_eq!(
            pick(&["--join", link, "--live-server", "https://other.example"]),
            pick(&["--live-server", "https://other.example"]),
        );
        assert!(parse(&["--join", "https://other.example"]).is_err());
        assert!(parse(&["--join"]).is_err());
        assert!(parse(&["--join", link, "--join", link]).is_err());
        assert!(parse(&["--join", link, "--offline"]).is_err());
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
        assert_eq!(session.account.as_ref().unwrap().gamertag, "test user");
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
        assert_eq!(handed[0].xuid, session.account.as_ref().unwrap().xuid);

        // The same server again: the account is found, not made.
        let client = Client::at(&session.server).unwrap();
        let (again, created) = bootstrap(
            &client,
            &places.accounts,
            None,
            "test.user",
            None,
            true,
            None,
        )
        .unwrap();
        assert!(!created);
        assert_eq!(again.xuid, session.account.as_ref().unwrap().xuid);
        // Another name is made beside it; a taken default is numbered.
        let (other, created) = bootstrap(
            &client,
            &places.accounts,
            Some("Other"),
            "x",
            None,
            true,
            None,
        )
        .unwrap();
        assert!(created && other.gamertag == "Other");
        let elsewhere = dir.join("elsewhere.json");
        let (numbered, _) =
            bootstrap(&client, &elsewhere, None, "test.user", None, true, None).unwrap();
        assert_eq!(numbered.gamertag, "test user2");
        // A chosen name someone else has is an error.
        assert!(bootstrap(&client, &elsewhere, Some("Other"), "x", None, true, None).is_err());
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
        assert_eq!(session.account.as_ref().unwrap().gamertag, "test user");
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
        let key = session.account.as_ref().unwrap().key_pair().unwrap();
        assert_eq!(
            entry,
            crate::live_admin::export_entry(
                session.account.as_ref().unwrap(),
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
