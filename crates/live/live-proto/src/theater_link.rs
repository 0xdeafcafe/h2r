//! Typed, credential-free Theater targets from the Live website.
use crate::{game::valid_id, join_link::server_base};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TheaterLink {
    server: String,
    pub film: String,
    pub clip: Option<String>,
    pub tick: Option<u32>,
    pub seat: Option<u8>,
    pub camera: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Invalid Theater link field: {0}")]
pub struct Invalid(pub &'static str);

impl FromStr for TheaterLink {
    type Err = Invalid;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let rest = raw
            .strip_prefix("h2recomp://theater/")
            .ok_or(Invalid("action"))?;
        if raw.len() > 4096 || raw.contains('#') || raw.bytes().any(|b| !b.is_ascii_graphic()) {
            return Err(Invalid("url"));
        }
        let (film, query) = rest.split_once('?').unwrap_or((rest, ""));
        if !valid_id(film) {
            return Err(Invalid("film"));
        }
        let mut link = Self {
            server: String::new(),
            film: film.into(),
            clip: None,
            tick: None,
            seat: None,
            camera: None,
        };
        for pair in query.split('&').filter(|_| !query.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or(Invalid("query"))?;
            match key {
                "server" => {
                    if !link.server.is_empty() {
                        return Err(Invalid("server"));
                    }
                    let (_, decoded) = url::form_urlencoded::parse(pair.as_bytes())
                        .next()
                        .ok_or(Invalid("server"))?;
                    link.server = server_base(&decoded).map_err(|_| Invalid("server"))?;
                }
                "clip" => {
                    if link.clip.is_some() || !valid_id(value) {
                        return Err(Invalid("clip"));
                    }
                    link.clip = Some(value.into());
                }
                "t" => {
                    if link.tick.is_some() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(Invalid("t"));
                    }
                    link.tick = Some(value.parse().map_err(|_| Invalid("t"))?);
                }
                "seat" => {
                    if link.seat.is_some() || !value.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(Invalid("seat"));
                    }
                    link.seat = Some(
                        value
                            .parse::<u8>()
                            .ok()
                            .filter(|n| *n < 16)
                            .ok_or(Invalid("seat"))?,
                    );
                }
                "camera" => {
                    if link.camera.is_some()
                        || !matches!(
                            value,
                            "default" | "first-person" | "follow" | "orbit" | "free"
                        )
                    {
                        return Err(Invalid("camera"));
                    }
                    link.camera = Some(value.into());
                }
                _ => return Err(Invalid("query")),
            }
        }
        if link.server.is_empty() {
            return Err(Invalid("server"));
        }
        Ok(link)
    }
}

impl TheaterLink {
    pub fn check_server(&self, current: &str) -> Result<(), Invalid> {
        if server_base(current).ok().as_ref() == Some(&self.server) {
            Ok(())
        } else {
            Err(Invalid("server"))
        }
    }

    pub fn check_bounds(
        &self,
        last_tick: u64,
        seats: impl Iterator<Item = u8>,
        clip: Option<[u64; 2]>,
    ) -> Result<(), Invalid> {
        if let Some([start, end]) = clip
            && (start >= end || end > last_tick.saturating_add(1))
        {
            return Err(Invalid("clip"));
        }
        if let Some(tick) = self.tick.map(u64::from)
            && (tick > last_tick || clip.is_some_and(|[start, end]| !(start..end).contains(&tick)))
        {
            return Err(Invalid("t"));
        }
        if self
            .seat
            .is_some_and(|seat| !seats.into_iter().any(|n| seat == n))
        {
            return Err(Invalid("seat"));
        }
        Ok(())
    }
}

/// Construct a web link only from a configured server and validated catalogue IDs.
pub fn web_link(server: &str, film: &str, clip: Option<&str>) -> Result<String, Invalid> {
    let base = server_base(server).map_err(|_| Invalid("server"))?;
    if !valid_id(film) {
        return Err(Invalid("film"));
    }
    let mut url = format!("{base}/theater/{film}");
    if let Some(id) = clip {
        if !valid_id(id) {
            return Err(Invalid("clip"));
        }
        url.push_str("/clip/");
        url.push_str(id);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: &str = "h2recomp://theater/game_1?server=https%3A%2F%2FLIVE.example%3A443";
    #[test]
    fn refuses_malformed_or_duplicate_fields_by_name() {
        for bad in [
            "https://theater/f",
            "h2recomp://theater:80/f",
            "h2recomp://theater@evil/f",
            "h2recomp://join/f",
        ] {
            assert_eq!(bad.parse::<TheaterLink>(), Err(Invalid("action")));
        }
        assert_eq!(
            format!("{BASE}#x").parse::<TheaterLink>(),
            Err(Invalid("url"))
        );
        assert_eq!(
            format!("{BASE}&unknown=1").parse::<TheaterLink>(),
            Err(Invalid("query"))
        );
        assert_eq!(
            format!("{BASE}&t").parse::<TheaterLink>(),
            Err(Invalid("query"))
        );
        for (field, values) in [
            ("clip", vec!["", ".", "../a", "%2F", "a/b", "abc.xyz"]),
            ("seat", vec!["", "16", "256", "-1", "+2", "1.0", "%31"]),
            ("t", vec!["", "-1", "+1", "4294967296", "1e2", "%30"]),
            ("camera", vec!["", "first/person", "raw"]),
        ] {
            for value in values {
                assert_eq!(
                    format!("{BASE}&{field}={value}").parse::<TheaterLink>(),
                    Err(Invalid(field))
                );
            }
        }
        for pair in ["clip=c", "seat=0", "camera=free", "t=0"] {
            assert_eq!(
                format!("{BASE}&{pair}&{pair}").parse::<TheaterLink>(),
                Err(Invalid(pair.split_once('=').unwrap().0))
            );
        }
        for id in ["", ".", "..", "%2e%2e", "a/b", "a@b", &"a".repeat(65)] {
            assert_eq!(
                BASE.replace("game_1", id).parse::<TheaterLink>(),
                Err(Invalid("film"))
            );
        }
        assert_eq!(
            format!("{BASE}&clip={}", "a".repeat(65)).parse::<TheaterLink>(),
            Err(Invalid("clip"))
        );
        assert_eq!(
            format!("{BASE}&server=x").parse::<TheaterLink>(),
            Err(Invalid("server"))
        );
        assert_eq!(
            "h2recomp://theater/g".parse::<TheaterLink>(),
            Err(Invalid("server"))
        );
        for server in [
            "",
            "ftp://a",
            "https:///a",
            "https://a@b",
            "https://a/%252e",
            "https://a/../b",
            "https://a%3Fx",
            "https://a%23x",
            "https://a%5Cb",
        ] {
            assert_eq!(
                format!("h2recomp://theater/g?server={server}").parse::<TheaterLink>(),
                Err(Invalid("server"))
            );
        }
    }

    #[test]
    fn validates_against_the_selected_server_roster_and_clip() {
        let link: TheaterLink = format!("{BASE}&t=5&seat=3&camera=orbit").parse().unwrap();
        assert!(link.check_server("https://live.example/").is_ok());
        for other in [
            "http://live.example",
            "https://other.example",
            "https://live.example:444",
            "https://live.example/sub",
        ] {
            assert_eq!(link.check_server(other), Err(Invalid("server")));
        }
        assert!(link.check_bounds(5, [3].into_iter(), Some([1, 6])).is_ok());
        assert_eq!(
            link.check_bounds(4, [3].into_iter(), None),
            Err(Invalid("t"))
        );
        assert_eq!(
            link.check_bounds(6, [0, 1].into_iter(), None),
            Err(Invalid("seat"))
        );
        assert_eq!(
            link.check_bounds(6, [3].into_iter(), Some([1, 5])),
            Err(Invalid("t"))
        );
        assert_eq!(
            link.check_bounds(6, [3].into_iter(), Some([2, 2])),
            Err(Invalid("clip"))
        );
    }
}
