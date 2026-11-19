//! What the Worker answers itself, ahead of the hub: the game art at
//! `/art/game/*`, composed emblems at `/art/emblem/*`, the stats site's old addresses (`/stats/*`, a 301 to
//! the same page under `/games/halo2`, in entry.rs), and the app's pages
//! (the built `index.html` with each page's link preview
//! [`live_server::page_meta`], and a game page's pre-rendered header
//! [`live_server::page_header`], which the hub's answer carries).
//!
//! The Worker has no art files (they are the owner's, extracted from their
//! disc). Art published to a public bucket (`h2 live assets publish`,
//! docs/live/live-worker.md, "Deploy") is a redirect there, as on a
//! machine's server with `--public-blobs`; with none published, a JSON
//! 404, so the web app uses its own art pack. Static assets never see
//! these paths: wrangler.toml sends `/art/game/*` to the Worker first.

use live_server::art::{PREFIX, published_redirect};

/// Whether to serve a composed image at the edge.
pub fn is_emblem(path: &str) -> bool {
    path.strip_prefix(live_server::emblem::PREFIX)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// What to answer a request under `/art/game`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameArt {
    Redirect {
        to: String,
        cache_control: &'static str,
    },
    NotFound(&'static str),
}

/// The answer for `path` (and its `query`) when it is under
/// [`PREFIX`], given `base` ([`crate::settings::GAME_ART_URL`]), else
/// `None`: the request is the hub's.
pub fn game_art(base: Option<&str>, path: &str, query: Option<&str>) -> Option<GameArt> {
    let rest = path.strip_prefix(PREFIX)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let Some(base) = base.map(str::trim).filter(|b| !b.is_empty()) else {
        return Some(GameArt::NotFound("no game art on this server"));
    };
    Some(match published_redirect(base, path, query) {
        Some((to, cache_control)) => GameArt::Redirect { to, cache_control },
        None => GameArt::NotFound("no such game art"),
    })
}

/// The hub's answer to "what is this page" for its link preview, when
/// the page names a player, game or clan; `None` when the path alone
/// says (so the hub is not woken for it).
pub fn page_meta_url(path: &str) -> Option<String> {
    live_server::page_meta::subject(path)?;
    Some(format!(
        "https://hub/v1/page-meta?path={}",
        encode_query(path)
    ))
}

/// `s` for a query value: unreserved characters and `/` as they are,
/// everything else percent-encoded.
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wrangler.toml sends the Worker exactly the app's pages (and the
    /// paths it had before): every page pattern is a page, and every kind
    /// of page has a pattern.
    #[test]
    fn wrangler_sends_the_worker_the_app_pages() {
        let toml = include_str!("../wrangler.toml");
        let list = toml.split("run_worker_first = [").nth(1).unwrap();
        let list = &list[..list.find(']').unwrap()];
        let patterns: Vec<&str> = list
            .split(',')
            .map(|p| p.trim().trim_matches('"'))
            .filter(|p| !p.is_empty())
            .collect();
        let api = [
            "/v1/*",
            "/.well-known/*",
            "/healthz",
            "/art/game",
            "/art/game/*",
            "/art/emblem",
            "/art/emblem/*",
        ];
        for p in &patterns {
            let sample = p.replace('*', "x");
            assert!(
                api.contains(p) || live_server::page_meta::is_page(&sample),
                "{p} is sent to the Worker but is not a page"
            );
        }
        for path in ["/art/emblem", "/art/emblem/1-2-3-4-5-6.png"] {
            assert!(is_emblem(path));
            assert!(patterns.iter().any(|p| {
                p.strip_suffix('*')
                    .map_or(*p == path, |prefix| path.starts_with(prefix))
            }));
        }
        assert!(!is_emblem("/art/emblems/01-double-arch.webp"));
        for page in [
            "/",
            "/dash",
            "/dash/x",
            "/stats",
            "/stats/x",
            "/players",
            "/players/x",
            "/games",
            "/games/x",
            "/games/halo2",
            "/games/halo2/players/x",
            "/events",
            "/signin",
        ] {
            let covered = patterns.iter().any(|p| match p.strip_suffix('*') {
                Some(prefix) => page.starts_with(prefix),
                None => *p == page,
            });
            assert!(
                covered,
                "{page} is a page but wrangler.toml serves it without the Worker"
            );
        }
    }

    #[test]
    fn only_pages_naming_something_ask_the_hub() {
        assert_eq!(
            page_meta_url("/games/halo2/players/Two Words&x"),
            Some("https://hub/v1/page-meta?path=/games/halo2/players/Two%20Words%26x".into())
        );
        assert_eq!(
            page_meta_url("/games/halo2/games/0123"),
            Some("https://hub/v1/page-meta?path=/games/halo2/games/0123".into())
        );
        for path in [
            "/",
            "/dash/players",
            "/games",
            "/games/halo2",
            "/games/halo2/games",
            // Redirected before any preview.
            "/stats/players/x",
        ] {
            assert_eq!(page_meta_url(path), None, "{path}");
        }
    }

    #[test]
    fn art_is_a_redirect_when_published_and_a_404_when_not() {
        let base = Some("https://art.example.net/art/0123456789abcdef");
        assert_eq!(
            game_art(base, "/art/game/manifest.json", None),
            Some(GameArt::Redirect {
                to: "https://art.example.net/art/0123456789abcdef/manifest.json".into(),
                cache_control: "public, max-age=300",
            })
        );
        assert_eq!(
            game_art(base, "/art/game/map/x/map.glb", Some("v=1")),
            Some(GameArt::Redirect {
                to: "https://art.example.net/art/0123456789abcdef/map/x/map.glb".into(),
                cache_control: "public, max-age=31536000, immutable",
            })
        );
        assert_eq!(
            game_art(base, "/art/game/../x", None),
            Some(GameArt::NotFound("no such game art"))
        );
        assert_eq!(
            game_art(base, "/art/game", None),
            Some(GameArt::NotFound("no such game art"))
        );
        for none in [None, Some(""), Some("  ")] {
            assert_eq!(
                game_art(none, "/art/game/manifest.json", None),
                Some(GameArt::NotFound("no game art on this server"))
            );
        }
        // Not the game art: the web's own art pack, the API.
        assert_eq!(game_art(base, "/art/medals/x.png", None), None);
        assert_eq!(game_art(base, "/art/gamex/a.png", None), None);
        assert_eq!(game_art(base, "/v1/games", None), None);
    }
}
