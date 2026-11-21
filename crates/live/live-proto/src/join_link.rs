//! The credential-free URL the Live website hands to the installed game.
use std::str::FromStr;
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct JoinLink {
    server: String,
    session: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LinkError {
    #[error("Expected a Halo 2 session link with the h2recomp://join action.")]
    Action,
    #[error("The join link must contain exactly one server and one session.")]
    Parameters,
    #[error(
        "The join link needs an HTTP or HTTPS Live server address without credentials, a query, or a fragment."
    )]
    Server,
    #[error("The session ID must contain 16 lowercase hexadecimal digits.")]
    Session,
}

impl JoinLink {
    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    /// Compares against the player's chosen Live base, including scheme,
    /// effective port and deployment path. The URL itself selects no service.
    pub fn matches_server(&self, current: &str) -> bool {
        server_base(current).is_ok_and(|base| base == self.server)
    }
}

fn server_base(raw: &str) -> Result<String, LinkError> {
    // Reject spellings a browser's permissive URL parser silently repairs
    // (an empty authority, backslashes, ignored whitespace or dot segments).
    let (_, rest) = raw.split_once("://").ok_or(LinkError::Server)?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.is_empty()
        || authority.contains(['@', '%'])
        || raw.bytes().any(|b| !b.is_ascii_graphic())
        || raw.contains(['\\', '?', '#'])
        || !path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~/".contains(&b))
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(LinkError::Server);
    }
    let url = Url::parse(raw).map_err(|_| LinkError::Server)?;
    if !matches!(url.scheme(), "https" | "http") || url.host().is_none() {
        return Err(LinkError::Server);
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

impl FromStr for JoinLink {
    type Err = LinkError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let query = raw
            .strip_prefix("h2recomp://join?")
            .ok_or(LinkError::Action)?;
        if raw.len() > 4096 || raw.contains('#') || raw.bytes().any(|b| !b.is_ascii_graphic()) {
            return Err(LinkError::Parameters);
        }
        let mut server = None;
        let mut session = None;
        for field in query.split('&') {
            let (key, value) = field.split_once('=').ok_or(LinkError::Parameters)?;
            match key {
                "server" if server.is_none() => {
                    // Decode just the query value, exactly once. A malformed
                    // or nested escape cannot be part of a valid base address.
                    let (_, value) = url::form_urlencoded::parse(field.as_bytes())
                        .next()
                        .ok_or(LinkError::Server)?;
                    server = Some(server_base(&value)?);
                }
                "session" if session.is_none() => {
                    if !crate::session::valid_id(value) {
                        return Err(LinkError::Session);
                    }
                    session = Some(value.to_owned());
                }
                _ => return Err(LinkError::Parameters),
            }
        }
        Ok(Self {
            server: server.ok_or(LinkError::Parameters)?,
            session: session.ok_or(LinkError::Parameters)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str =
        "h2recomp://join?server=https%3A%2F%2FLIVE.example%3A443%2Fhub%2F&session=800000000000abcd";

    #[test]
    fn same_service_includes_scheme_port_and_path() {
        let link: JoinLink = LINK.parse().unwrap();
        assert_eq!(link.server(), "https://live.example/hub");
        assert_eq!(link.session(), "800000000000abcd");
        assert!(link.matches_server("https://live.example:443/hub/"));
        for other in [
            "https://other.example/hub",
            "https://live.example",
            "http://live.example/hub",
            "https://live.example:4430/hub",
        ] {
            assert!(!link.matches_server(other));
        }
        let local: JoinLink =
            "h2recomp://join?session=800000000000abcd&server=http%3A%2F%2F%5B%3A%3A1%5D%3A8074"
                .parse()
                .unwrap();
        assert!(local.matches_server("http://[::1]:8074"));
    }

    #[test]
    fn only_the_join_action_and_two_named_fields_are_accepted() {
        for raw in [
            LINK.replace("//join?", "//other?"),
            LINK.replace("//join?", "//join/?"),
            LINK.replace("//join?", "//name@join?"),
            LINK.replace("//join?", "//join:80?"),
            LINK.replace("h2recomp:", "https:"),
            format!("{LINK}#"),
            format!("{LINK}&"),
            format!("{LINK}&server=https://other.example"),
            format!("{LINK}&key=value"),
            format!("{LINK}&session=800000000000abcd"),
            LINK.replace("&session=", "&other="),
            LINK.replace("800000000000abcd", ""),
            LINK.replace("800000000000abcd", "../foo"),
            LINK.replace("800000000000abcd", "800000000000ABCD"),
            LINK.repeat(150),
        ] {
            assert!(raw.parse::<JoinLink>().is_err(), "{raw}");
        }
    }

    #[test]
    fn rejects_unexpected_server_components_before_network_use() {
        for raw in [
            "ftp://live.example",
            "file:///host",
            "https:live.example",
            "https:///odd",
            "https://me:password@live.example",
            "https://@live.example",
            "https://a/b?query",
            "https://a/#secret",
            "https://a/x/../y",
            "https://a/%2f",
            "https://a\\b",
            "https://a/\n",
            "https://",
            "https://%61",
            "https://example.org:99999",
        ] {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("server", raw)
                .append_pair("session", "800000000000abcd")
                .finish();
            assert_eq!(
                format!("h2recomp://join?{query}").parse::<JoinLink>(),
                Err(LinkError::Server),
                "{raw:?}"
            );
        }
    }
}
