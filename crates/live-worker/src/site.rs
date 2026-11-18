//! What the Worker answers itself, ahead of the hub: the game art at
//! `/art/game/*`.
//!
//! The Worker has no art files (they are the owner's, extracted from their
//! disc). Art published to a public bucket (`h2 live assets publish`,
//! docs/live/live-worker.md, "Deploy") is a redirect there, as on a
//! machine's server with `--public-blobs`; with none published, a JSON
//! 404, so the web app uses its own art pack. Static assets never see
//! these paths: wrangler.toml sends `/art/game/*` to the Worker first.

use live_server::art::{PREFIX, published_redirect};

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

#[cfg(test)]
mod tests {
    use super::*;

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
