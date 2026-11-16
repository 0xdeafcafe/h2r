//! `h2-live-server`: the service alone, for a container
//! (docs/live/live-hosting.md, "Docker with a named tunnel"). The same server as
//! `h2 live serve`, built without the rest of `h2` (the recompiler, the
//! renderer, the disc tools), so it builds on Linux and the image stays
//! small. It does not run cloudflared: a tunnel beside it does
//! (`--behind-cloudflared`), and the art is extracted beforehand with `h2`
//! on a machine with the disc.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::art::GameArt;
use crate::{Config, Policy, http, identity, public};

pub const USAGE: &str = "usage: h2-live-server --data DIR [options]
       h2-live-server store copy --from DB --to DB [--from-blobs LOC --to-blobs LOC]

  --data DIR              the server's key and id (fixed on first start);
                          $H2_LIVE_DATA if not given
  --db URL                the event log: sqlite://PATH or postgres://...; else
                          $H2_LIVE_DB, else sqlite://DIR/live.db (or, for a
                          server that has no live.db yet but a log in
                          postgres://localhost/h2live, that, with a notice)
  --blobs LOC             content and cloud saves: DIR (the data directory by
                          default) or s3://BUCKET[/PREFIX] (R2: endpoint and
                          keys from $H2_LIVE_S3_ENDPOINT, $H2_LIVE_S3_ACCESS_KEY_ID,
                          $H2_LIVE_S3_SECRET_ACCESS_KEY); $H2_LIVE_BLOBS
  --public-blobs LOC      a second, public bucket for shared game art, which
  --public-blobs-url URL  is then redirected to at URL (the bucket's custom
                          domain or r2.dev address); $H2_LIVE_PUBLIC_BLOBS,
                          $H2_LIVE_PUBLIC_BLOBS_URL
  --listen ADDR           default 127.0.0.1:8074; a non-loopback address
                          needs --tls-cert/--tls-key or --insecure-http
  --id HOST               the server id on first start
  --private               reads need a token
  --invite-only           account creation needs an invite
  --admin GAMERTAG        an admin (repeatable; also $H2_LIVE_ADMINS)
  --game-art DIR          art from `h2 live assets extract` (default DIR/game-art
                          under --data if extracted there), served at /art/game/
  --share-game-art        serve the art to everyone, not only this machine
  --public-url URL        the URL clients outside reach the server at
  --behind-cloudflared    that URL is a Cloudflare tunnel whose cloudflared
                          reaches this server on loopback: CF-Connecting-IP
                          names the client, and the URL's host is the id on
                          first start
  --tls-cert PEM --tls-key PEM | --insecure-http
  --content-layer-mib N   largest uploaded patch, pack or image (default 32)
  --content-map-mib N     largest uploaded whole map (default 100)
  --content-quota-mib N   content one account may store (default 1024)

Retail map hashes the content store refuses: DIR/retail-maps.sha256
(sha256sum output; docs/live/live-admin.md, \"Custom content\").
The web app is served from $H2_LIVE_WEB (a built web/dist).";

/// What `h2-live-server` was asked to run, and the lines to print first.
pub struct Standalone {
    pub config: Config,
    pub notices: Vec<String>,
}

/// Parses `h2-live-server`'s arguments. `env` reads the environment
/// (`H2_LIVE_DATA`, `H2_LIVE_DB`, `H2_LIVE_WEB`; `H2_LIVE_ADMINS` is read by
/// [`crate::admin::Admins::from_flags_and_env`]).
pub fn parse(args: Vec<String>, env: impl Fn(&str) -> Option<String>) -> Result<Standalone> {
    let mut args = Args(args);
    if args.flag("--help") || args.flag("-h") {
        bail!("{USAGE}");
    }
    let data_dir: PathBuf = args
        .take("--data")?
        .or_else(|| env("H2_LIVE_DATA"))
        .with_context(|| format!("--data DIR is needed\n\n{USAGE}"))?
        .into();
    let db = args.take("--db")?.or_else(|| env("H2_LIVE_DB"));
    let blobs = crate::BlobFlags {
        private: args.take("--blobs")?,
        public: args.take("--public-blobs")?,
        public_url: args.take("--public-blobs-url")?,
    }
    .or_env(&env);
    let listen: SocketAddr = args
        .take("--listen")?
        .unwrap_or_else(|| crate::DEFAULT_LISTEN.into())
        .parse()
        .with_context(|| {
            format!(
                "bad --listen address (want ip:port like {})",
                crate::DEFAULT_LISTEN
            )
        })?;
    let id = args.take("--id")?;
    let mut admin_flags = Vec::new();
    while let Some(a) = args.take("--admin")? {
        admin_flags.push(a);
    }
    let tls = match (args.take("--tls-cert")?, args.take("--tls-key")?) {
        (Some(cert), Some(key)) => Some((cert.into(), key.into())),
        (None, None) => None,
        _ => bail!("--tls-cert and --tls-key go together"),
    };
    let policy = Policy {
        private: args.flag("--private"),
        invite_only: args.flag("--invite-only"),
        admins: crate::admin::Admins::from_flags_and_env(&admin_flags)
            .map_err(|e| anyhow::anyhow!("--admin/H2_LIVE_ADMINS: {e}"))?,
        content: crate::content::ContentLimits::from_flags(
            args.take("--content-layer-mib")?,
            args.take("--content-map-mib")?,
            args.take("--content-quota-mib")?,
        )
        .map_err(anyhow::Error::msg)?,
        ..Policy::default()
    };
    let insecure_http = args.flag("--insecure-http");
    let art_dir = args.take("--game-art")?.map(PathBuf::from);
    let share_art = args.flag("--share-game-art");
    let public_url = args.take("--public-url")?;
    let behind_cloudflared = args.flag("--behind-cloudflared");
    if let Some(extra) = args.0.first() {
        bail!("unexpected {extra}\n\n{USAGE}");
    }

    let mut notices = Vec::new();
    let mut lasting = None;
    match (&public_url, behind_cloudflared) {
        (None, true) => {
            bail!("--behind-cloudflared needs --public-url https://HOST (the tunnel's host name)")
        }
        (Some(url), cloudflared) => {
            public::check_public_url(url)?;
            policy.public.set(url, cloudflared);
            if cloudflared {
                lasting = public::lasting_host(url);
                notices.push(format!("public at {url} (through cloudflared)"));
                notices.extend(public::exposure_notices(policy.invite_only, policy.private));
            }
        }
        (None, false) => {}
    }
    let id = identity::first_start_id(id, lasting, &data_dir);
    let game_art = GameArt::from_flag(art_dir, &data_dir.join("game-art"), share_art)?;
    let web = env("H2_LIVE_WEB")
        .map(PathBuf::from)
        .filter(|d| d.join("index.html").is_file());
    Ok(Standalone {
        config: Config {
            db,
            blobs,
            listen,
            data_dir,
            id,
            policy,
            tls,
            insecure_http,
            site: http::Site { web, game_art },
        },
        notices,
    })
}

pub(crate) struct Args(pub(crate) Vec<String>);

impl Args {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "h2live-standalone-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn run(args: &[&str], env: &[(&str, &str)]) -> Result<Standalone> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        parse(args.iter().map(|s| s.to_string()).collect(), move |k| {
            env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        })
    }

    /// What deploy/live/compose.yaml runs.
    #[test]
    fn the_compose_command_line() {
        let dir = temp_dir("compose");
        let data = dir.join("data");
        let art = dir.join("art");
        let web = dir.join("web");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(art.join("manifest.json"), "{}").unwrap();
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(web.join("index.html"), "<!doctype html>").unwrap();
        let s = run(
            &[
                "--listen",
                "127.0.0.1:8074",
                "--data",
                data.to_str().unwrap(),
                "--game-art",
                art.to_str().unwrap(),
                "--share-game-art",
                "--public-url",
                "https://live.forbes.red",
                "--behind-cloudflared",
                "--invite-only",
            ],
            &[
                ("H2_LIVE_DB", "postgres://h2live:pw@db/h2live"),
                ("H2_LIVE_WEB", web.to_str().unwrap()),
            ],
        )
        .unwrap();
        let c = &s.config;
        assert_eq!(c.db.as_deref(), Some("postgres://h2live:pw@db/h2live"));
        assert_eq!(c.blobs, crate::BlobFlags::default());
        assert!(
            c.listen.ip().is_loopback(),
            "the tunnel shares the namespace"
        );
        assert_eq!(c.id.as_deref(), Some("live.forbes.red"), "a new server");
        assert!(c.policy.invite_only);
        assert!(!c.policy.private);
        assert_eq!(
            c.policy.public.get_with_kind(),
            Some(("https://live.forbes.red".to_string(), true))
        );
        assert_eq!(c.site.web.as_deref(), Some(web.as_path()));
        let game_art = c.site.game_art.as_ref().unwrap();
        assert_eq!(game_art.dir, art);
        assert!(game_art.share);
        assert!(s.notices[0].contains("https://live.forbes.red"));
        assert!(
            s.notices.iter().any(|n| n.contains("reads are public")),
            "{:?}",
            s.notices
        );
        assert!(crate::check_bind(c.listen, c.tls.is_some(), c.insecure_http).is_ok());

        // A restart keeps the id the first start fixed.
        crate::ServerIdentity::open(&data, c.id.as_deref()).unwrap();
        let again = run(
            &[
                "--data",
                data.to_str().unwrap(),
                "--public-url",
                "https://live.forbes.red",
                "--behind-cloudflared",
            ],
            &[],
        )
        .unwrap();
        assert_eq!(again.config.id, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plain_public_url_trusts_no_cloudflare_header_and_names_nothing() {
        let dir = temp_dir("proxy");
        let s = run(
            &["--public-url", "https://live.example.net"],
            &[("H2_LIVE_DATA", dir.to_str().unwrap())],
        )
        .unwrap();
        assert_eq!(s.config.data_dir, dir);
        assert_eq!(s.config.id, None);
        assert_eq!(
            s.config.policy.public.get_with_kind(),
            Some(("https://live.example.net".to_string(), false))
        );
        assert!(s.notices.is_empty());
        assert_eq!(s.config.db, None, "resolve_db picks at startup");
        assert_eq!(s.config.site.web, None, "no $H2_LIVE_WEB");
        assert_eq!(s.config.site.game_art, None, "nothing extracted");
    }

    /// The free-hosting command line: SQLite, R2 for blobs, a public bucket
    /// for the art.
    #[test]
    fn sqlite_and_r2_flags() {
        let s = run(
            &[
                "--data",
                "/srv/live",
                "--db",
                "sqlite:///srv/live/live.db",
                "--blobs",
                "s3://h2live",
                "--public-blobs-url",
                "https://art.example.net",
            ],
            &[("H2_LIVE_PUBLIC_BLOBS", "s3://h2live-public")],
        )
        .unwrap();
        assert_eq!(s.config.db.as_deref(), Some("sqlite:///srv/live/live.db"));
        assert_eq!(
            s.config.blobs,
            crate::BlobFlags {
                private: Some("s3://h2live".into()),
                public: Some("s3://h2live-public".into()),
                public_url: Some("https://art.example.net".into()),
            }
        );
    }

    #[test]
    fn bad_command_lines() {
        let err = |args: &[&str]| match run(args, &[]) {
            Ok(_) => panic!("{args:?} was accepted"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err(&[]).contains("--data DIR is needed"));
        assert!(err(&["--data", "/x", "--behind-cloudflared"]).contains("needs --public-url"));
        assert!(err(&["--data", "/x", "--public-url", "live.example"]).contains("wants a URL"));
        assert!(err(&["--data", "/x", "--tls-cert", "a.pem"]).contains("go together"));
        assert!(err(&["--data", "/x", "--listen", "nowhere"]).contains("bad --listen"));
        assert!(err(&["--data", "/x", "--tunnel"]).contains("unexpected --tunnel"));
        assert!(err(&["--data", "/x", "--admin", "not a valid tag!!"]).contains("--admin"));
        assert!(err(&["--data"]).contains("needs a value"));
        assert!(err(&["--help"]).starts_with("usage: h2-live-server"));
        let missing = std::env::temp_dir().join("h2live-standalone-no-such-art");
        assert!(
            err(&["--data", "/x", "--game-art", missing.to_str().unwrap()])
                .contains("no manifest.json")
        );
    }
}
