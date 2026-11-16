//! Serving through a proxy or tunnel (docs/live-hosting.md): the public
//! base URL the server is reached at, and how to tell a request a proxy
//! forwarded from one made on this machine.
//!
//! A tunnel such as cloudflared connects to the server from loopback, so
//! the peer address alone says "this machine" for every request from the
//! internet. What gives a forwarded request away is the headers the proxy
//! adds ([`forwarded`]); this-machine-only routes (game art, local sign-in)
//! refuse any request that carries one.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};

use axum::http::HeaderMap;

/// The base URL clients outside this machine use, if the server knows one
/// (`h2 live serve --tunnel`, `--public-url`). Shared: a quick tunnel
/// learns its URL after the server starts and gets a new one when it
/// restarts, so whoever runs the tunnel sets it while the server runs.
#[derive(Debug, Clone, Default)]
pub struct PublicUrl {
    inner: Arc<RwLock<Option<Public>>>,
}

#[derive(Debug, Clone)]
struct Public {
    url: String,
    cloudflare: bool,
}

impl PublicUrl {
    /// Sets the URL (no trailing slash). `cloudflare`: requests arrive
    /// through a Cloudflare tunnel, so `CF-Connecting-IP` names the client
    /// ([`PublicUrl::client_ip`]).
    pub fn set(&self, url: &str, cloudflare: bool) {
        *self.inner.write().unwrap() = Some(Public {
            url: url.trim_end_matches('/').to_string(),
            cloudflare,
        });
    }

    pub fn clear(&self) {
        *self.inner.write().unwrap() = None;
    }

    pub fn get(&self) -> Option<String> {
        self.inner.read().unwrap().as_ref().map(|p| p.url.clone())
    }

    /// The URL and whether it is a Cloudflare tunnel's.
    pub fn get_with_kind(&self) -> Option<(String, bool)> {
        self.inner
            .read()
            .unwrap()
            .as_ref()
            .map(|p| (p.url.clone(), p.cloudflare))
    }

    /// The client's address: `CF-Connecting-IP` when the server is behind a
    /// Cloudflare tunnel and the request came through it (from loopback),
    /// else the peer. Without a tunnel the header is anyone's to forge, so
    /// it is ignored.
    pub fn client_ip(&self, peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<IpAddr> {
        let peer = peer?;
        let cloudflare = self
            .inner
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|p| p.cloudflare);
        let forwarded_for = headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        match forwarded_for {
            Some(ip) if cloudflare && peer.ip().is_loopback() => Some(ip),
            _ => Some(peer.ip()),
        }
    }
}

impl crate::Live {
    /// Admin gamertags configured at startup (`--admin`, `H2_LIVE_ADMINS`)
    /// that no account holds yet. On a server the internet can reach,
    /// whoever registers one of those first is an admin, so `serve` and
    /// `play --tunnel` warn about them.
    pub fn unclaimed_admins(&self) -> Vec<String> {
        let state = self.model.read();
        self.admins
            .configured()
            .into_iter()
            .filter(|tag| {
                live_proto::Gamertag::new(tag)
                    .ok()
                    .is_none_or(|t| state.account_by_tag(&t).is_none())
            })
            .collect()
    }
}

/// The warning about [`crate::Live::unclaimed_admins`], if any.
pub fn unclaimed_admins_notice(tags: &[String]) -> Option<String> {
    (!tags.is_empty()).then(|| {
        format!(
            "configured admin gamertag(s) with no account yet: {}; whoever creates one first \
             is an admin. Create them now, or use --invite-only, or `h2 live admin grant` \
             an existing account instead",
            tags.join(", ")
        )
    })
}

/// Checks a `--public-url`: an absolute `http(s)://` URL.
pub fn check_public_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        anyhow::bail!("--public-url {url:?} wants a URL like https://live.example.net");
    }
    Ok(())
}

/// The host of a public URL that may name a new server
/// ([`crate::ServerIdentity`]'s id is fixed on first start): an `https://`
/// URL's host, lowercased, with no port, and not a quick tunnel's
/// (`*.trycloudflare.com` changes every run).
pub fn lasting_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    let quick = host == "trycloudflare.com" || host.ends_with(".trycloudflare.com");
    let plain = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    (plain && !quick).then_some(host)
}

/// What to tell the operator of a server the internet can reach about who
/// may sign up and read.
pub fn exposure_notices(invite_only: bool, private: bool) -> Vec<String> {
    let mut out = Vec::new();
    if !invite_only {
        out.push(
            "anyone with the URL can create an account; --invite-only limits that to invite codes"
                .into(),
        );
    }
    if !private {
        out.push("reads are public (games, accounts, the log); --private needs a token".into());
    }
    out
}

/// Whether a proxy says it forwarded the request for someone else: any
/// `Forwarded`, `X-Forwarded-*`, `X-Real-IP`, `True-Client-IP` or `CF-*`
/// header (cloudflared adds `CF-Connecting-IP`, `CF-Ray`, `CF-Visitor`
/// and `X-Forwarded-For`/`-Proto` to every request it delivers).
pub fn forwarded(headers: &HeaderMap) -> bool {
    headers.keys().any(|k| {
        let k = k.as_str(); // header names are lowercase
        k == "forwarded"
            || k == "x-real-ip"
            || k == "true-client-ip"
            || k.starts_with("x-forwarded-")
            || k.starts_with("cf-")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn proxy_headers_mark_a_request_forwarded() {
        assert!(!forwarded(&headers(&[
            ("host", "127.0.0.1:8074"),
            ("origin", "x")
        ])));
        for name in [
            "forwarded",
            "x-forwarded-for",
            "x-forwarded-proto",
            "x-forwarded-host",
            "x-real-ip",
            "true-client-ip",
            "cf-connecting-ip",
            "cf-ray",
            "cf-visitor",
            "cf-ipcountry",
            "cf-warp-tag-id",
        ] {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::HeaderName::from_static(name),
                "1".parse().unwrap(),
            );
            assert!(forwarded(&h), "{name}");
        }
        // Mixed case on the wire is lowercased by the parser.
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::HeaderName::from_bytes(b"CF-Connecting-IP").unwrap(),
            "203.0.113.9".parse().unwrap(),
        );
        assert!(forwarded(&h));
    }

    #[tokio::test]
    async fn configured_admins_without_accounts_are_unclaimed() {
        use live_proto::sign::{AccountRequest, KeyPair};
        let dir = std::env::temp_dir().join(format!(
            "h2live-unclaimed-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = crate::ServerIdentity::open(&dir, Some("live.test")).unwrap();
        let policy = crate::Policy {
            admins: vec![
                live_proto::Gamertag::new("afr").unwrap(),
                live_proto::Gamertag::new("Bob").unwrap(),
            ],
            ..crate::Policy::default()
        };
        let live = crate::Live::start(
            std::sync::Arc::new(crate::store::MemoryStore::new()),
            identity,
            policy,
        )
        .await
        .unwrap();
        let mut unclaimed = live.unclaimed_admins();
        unclaimed.sort();
        assert_eq!(unclaimed, ["Bob", "afr"]);
        let key = KeyPair::generate();
        let req = AccountRequest::sign(&key, "AFR", None, "live.test", crate::app::now());
        live.create_account(&req).await.unwrap();
        assert_eq!(live.unclaimed_admins(), ["Bob"]);
        assert!(
            unclaimed_admins_notice(&live.unclaimed_admins())
                .unwrap()
                .contains("Bob")
        );
        assert_eq!(unclaimed_admins_notice(&[]), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn public_urls_are_checked() {
        assert!(check_public_url("https://live.example.net").is_ok());
        assert!(check_public_url("http://192.168.1.5:8074").is_ok());
        assert!(check_public_url("live.example.net").is_err());
    }

    #[test]
    fn a_lasting_host_is_a_plain_https_host() {
        assert_eq!(
            lasting_host("https://Live.Forbes.Red/").as_deref(),
            Some("live.forbes.red")
        );
        assert_eq!(
            lasting_host("https://live.example.net/h2?x#y").as_deref(),
            Some("live.example.net")
        );
        assert_eq!(lasting_host("http://live.example.net"), None, "not https");
        assert_eq!(
            lasting_host("https://live.example.net:8443"),
            None,
            "a port"
        );
        assert_eq!(lasting_host("https://"), None);
        assert_eq!(lasting_host("https://user@live.example.net"), None);
        assert_eq!(
            lasting_host("https://seasonal-deck-organisms-sf.trycloudflare.com"),
            None,
            "a quick tunnel's URL changes every run"
        );
    }

    #[test]
    fn exposure_notices_name_what_is_open() {
        assert_eq!(exposure_notices(false, false).len(), 2);
        assert!(exposure_notices(true, false)[0].contains("reads are public"));
        assert!(exposure_notices(true, true).is_empty());
    }

    #[test]
    fn the_url_is_shared_and_replaceable() {
        let a = PublicUrl::default();
        let b = a.clone();
        assert_eq!(a.get(), None);
        b.set("https://one.trycloudflare.com/", true);
        assert_eq!(a.get().as_deref(), Some("https://one.trycloudflare.com"));
        b.set("https://two.trycloudflare.com", true);
        assert_eq!(a.get().as_deref(), Some("https://two.trycloudflare.com"));
        a.clear();
        assert_eq!(b.get(), None);
    }

    #[test]
    fn cf_connecting_ip_counts_only_behind_a_tunnel_and_from_loopback() {
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:5000".parse().unwrap();
        let h = headers(&[("cf-connecting-ip", "203.0.113.9")]);
        let p = PublicUrl::default();
        assert_eq!(
            p.client_ip(Some(lo), &h),
            Some(lo.ip()),
            "no tunnel: forged"
        );
        p.set("https://live.example", false);
        assert_eq!(p.client_ip(Some(lo), &h), Some(lo.ip()), "not cloudflare");
        p.set("https://x.trycloudflare.com", true);
        assert_eq!(
            p.client_ip(Some(lo), &h),
            Some("203.0.113.9".parse().unwrap())
        );
        assert_eq!(
            p.client_ip(Some(lan), &h),
            Some(lan.ip()),
            "not via the tunnel"
        );
        assert_eq!(p.client_ip(Some(lo), &HeaderMap::new()), Some(lo.ip()));
        assert_eq!(p.client_ip(None, &h), None);
    }
}
