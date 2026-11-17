//! Where a game listens and a client connects: a Unix socket path, or
//! `tcp://HOST:PORT` (the Xbox app, which has no Unix sockets and is
//! reached over the LAN).

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix socket at this path.
    Unix(PathBuf),
    /// A TCP address, `HOST:PORT` (`0.0.0.0:4720` to listen on every
    /// interface).
    Tcp(String),
}

impl Endpoint {
    /// `tcp://HOST:PORT` (or `tcp:HOST:PORT`) is TCP; anything else is a
    /// socket path. A TCP address must have a port.
    pub fn parse(s: &str) -> Result<Endpoint, String> {
        let tcp = s.strip_prefix("tcp://").or_else(|| s.strip_prefix("tcp:"));
        match tcp {
            Some(addr) => {
                let addr = addr.trim_end_matches('/');
                let port = addr.rsplit_once(':').map(|(_, p)| p);
                if addr.is_empty() || !port.is_some_and(|p| p.parse::<u16>().is_ok()) {
                    return Err(format!("{s}: expected tcp://HOST:PORT"));
                }
                Ok(Endpoint::Tcp(addr.to_owned()))
            }
            None if s.is_empty() => Err("an empty endpoint".into()),
            None => Ok(Endpoint::Unix(PathBuf::from(s))),
        }
    }
}

impl From<PathBuf> for Endpoint {
    fn from(p: PathBuf) -> Endpoint {
        match p.to_str().map(Endpoint::parse) {
            Some(Ok(e)) => e,
            _ => Endpoint::Unix(p),
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Unix(p) => write!(f, "{}", p.display()),
            Endpoint::Tcp(a) => write!(f, "tcp://{a}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_addresses_and_socket_paths() {
        assert_eq!(
            Endpoint::parse("tcp://10.0.3.29:4720"),
            Ok(Endpoint::Tcp("10.0.3.29:4720".into()))
        );
        assert_eq!(
            Endpoint::parse("tcp:0.0.0.0:4720/"),
            Ok(Endpoint::Tcp("0.0.0.0:4720".into()))
        );
        assert_eq!(
            Endpoint::parse("tcp://[::1]:9"),
            Ok(Endpoint::Tcp("[::1]:9".into()))
        );
        assert_eq!(
            Endpoint::parse("/tmp/agent.sock"),
            Ok(Endpoint::Unix("/tmp/agent.sock".into()))
        );
        for bad in [
            "tcp://",
            "tcp://host",
            "tcp://host:port",
            "tcp://h:99999",
            "",
        ] {
            assert!(Endpoint::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Endpoint::from(PathBuf::from("tcp://h:1")),
            Endpoint::Tcp("h:1".into())
        );
        assert_eq!(Endpoint::Tcp("h:1".into()).to_string(), "tcp://h:1");
    }
}
