//! `h2 live hosts start` (docs/live/dedicated-host.md): an admin starts a
//! custom dedicated host by hand. Dispatched from `live_cmd.rs`.
//!
//! The default asks the server to launch its configured provider. With
//! `--no-launch` the server only reserves the lease and answers its host
//! token — printed here as shell lines (`H2_LEASE`, `H2_HOST_LEASE_JSON`,
//! `H2_TOKEN`, `H2_LIVE`) to `eval` before the manual `docker run` in
//! [deploy/host/README.md](../../../../deploy/host/README.md). The token
//! appears nowhere else: not in other output, not in errors.

use anyhow::{Context, Result, bail, ensure};
use live_proto::hosts::{HostClass, Transport};
use serde_json::{Value, json};

use crate::live_cmd::{self, Args, Client};
use crate::live_recovery::Signer;

const USAGE: &str = "usage: h2 live hosts start --map MAP [--variant VARIANT]
       [--max-players N] [--class small|large] [--region REGION]
       [--transport udp|tunnel] [--no-launch] [--live URL]
       [--data DIR | --as GAMERTAG]";

/// The parsed command: the `POST /v1/admin/hosts` body (docs/live/live-api.md).
pub(crate) struct HostStart {
    map: String,
    variant: Option<String>,
    max_players: u8,
    class: HostClass,
    region: Option<String>,
    transport: Option<Transport>,
    launch: bool,
}

impl HostStart {
    pub(crate) fn parse(args: &mut Args) -> Result<Self> {
        let map = args
            .take("--map")?
            .with_context(|| format!("--map is required\n\n{USAGE}"))?;
        ensure!(
            (1..=64).contains(&map.len()) && map.is_ascii(),
            "--map must be 1 to 64 ASCII characters"
        );
        let variant = args.take("--variant")?;
        let max_players: u8 = args
            .take("--max-players")?
            .map_or(Ok(8), |n| n.parse())
            .context("--max-players must be a whole number")?;
        ensure!(
            (1..=16).contains(&max_players),
            "--max-players must be between 1 and 16"
        );
        let class = match args.take("--class")?.as_deref() {
            None => HostClass::Small,
            Some("small") => HostClass::Small,
            Some("large") => HostClass::Large,
            Some(other) => bail!("--class must be small or large, not {other:?}"),
        };
        let region = args.take("--region")?;
        let transport = match args.take("--transport")?.as_deref() {
            None => None,
            Some("udp") => Some(Transport::Udp),
            Some("tunnel") => Some(Transport::Tunnel),
            Some(other) => bail!("--transport must be udp or tunnel, not {other:?}"),
        };
        let launch = !args.flag("--no-launch");
        Ok(Self {
            map,
            variant,
            max_players,
            class,
            region,
            transport,
            launch,
        })
    }

    fn body(&self) -> Value {
        json!({
            "map": self.map,
            "variant": self.variant,
            "max_players": self.max_players,
            "class": self.class,
            "region": self.region,
            "transport": self.transport,
            "launch": self.launch,
        })
    }
}

pub fn start(mut args: Args) -> Result<()> {
    let live = args.take("--live")?;
    let request = HostStart::parse(&mut args)?;
    let client = Client::connect(live)?;
    let signer = Signer::from_args(&mut args, &client)?;
    args.positional_for(USAGE)?;
    let token = signer.token(&client, live_server::admin::LXM_ADMIN_WRITE)?;
    let req = client.agent.post(&client.url("/admin/hosts"));
    let body = request.body().to_string();
    let text = live_cmd::call(client.authorized(req, token)?, Some(body.as_bytes()))?;
    let reply: Value =
        serde_json::from_str(&text).context("the server's reply is not a host start")?;
    let lease = reply
        .get("lease")
        .filter(|l| l.is_object())
        .context("the server's reply has no lease")?;
    if !request.launch {
        let token = reply["token"]
            .as_str()
            .context("this server did not answer a host token for a --no-launch start")?;
        print!("{}", env_lines(&client.base, lease, token));
        eprintln!(
            "lease {id} is reserved and times out until its container registers; \
             eval the lines above before the docker run",
            id = lease["id"].as_str().unwrap_or("?")
        );
    } else {
        println!("{}", serde_json::to_string_pretty(lease)?);
    }
    Ok(())
}

/// The `--no-launch` output: `VAR=value` lines a shell can `eval` before the
/// manual `docker run` (which passes them with `-e`). Every value is either
/// bare (all characters inert in an assignment) or single-quoted with
/// embedded quotes escaped, so `eval` cannot split on, or inject through,
/// the lease JSON.
pub(crate) fn env_lines(live_url: &str, lease: &Value, token: &str) -> String {
    let id = lease["id"].as_str().expect("a lease has an id");
    format!(
        "H2_LEASE={}\nH2_HOST_LEASE_JSON={}\nH2_TOKEN={}\nH2_LIVE={}\n",
        shell_value(id),
        shell_value(&lease.to_string()),
        shell_value(token),
        shell_value(live_url),
    )
}

/// A value for a `VAR=value` line `eval` reads: bare when every character is
/// inert in an assignment, otherwise single-quoted with `'` as `'\''`.
pub(crate) fn shell_value(value: &str) -> String {
    let inert = |b: u8| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':' | b'/' | b'=' | b'~')
    };
    if !value.is_empty() && value.bytes().all(inert) {
        return value.to_owned();
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        if c == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(flags: &[&str]) -> Result<HostStart> {
        HostStart::parse(&mut Args(flags.iter().map(|s| s.to_string()).collect()))
    }

    #[test]
    fn parses_the_defaults_and_every_flag() {
        let start = parsed(&["--map", "lockout"]).unwrap();
        assert_eq!(start.map, "lockout");
        assert_eq!(start.max_players, 8);
        assert_eq!(start.class, HostClass::Small);
        assert!(start.variant.is_none());
        assert!(start.region.is_none());
        assert!(start.transport.is_none());
        assert!(start.launch);
        assert_eq!(
            start.body(),
            json!({
                "map": "lockout", "variant": null, "max_players": 8,
                "class": "small", "region": null, "transport": null, "launch": true,
            })
        );
        let full = parsed(&[
            "--map",
            "gemini",
            "--variant",
            "Slayer's Rift",
            "--max-players",
            "16",
            "--class",
            "large",
            "--region",
            "lhr",
            "--transport",
            "tunnel",
            "--no-launch",
        ])
        .unwrap();
        assert!(!full.launch);
        assert_eq!(
            full.body(),
            json!({
                "map": "gemini", "variant": "Slayer's Rift", "max_players": 16,
                "class": "large", "region": "lhr", "transport": "tunnel", "launch": false,
            })
        );
    }

    #[test]
    fn rejects_bad_arguments() {
        let long = "l".repeat(65);
        for bad in [
            vec![],
            vec!["--map"],
            vec!["--map", ""],
            vec!["--map", "lockoüt"],
            vec!["--map", long.as_str()],
            vec!["--map", "lockout", "--max-players", "0"],
            vec!["--map", "lockout", "--max-players", "17"],
            vec!["--map", "lockout", "--max-players", "eight"],
            vec!["--map", "lockout", "--class", "huge"],
            vec!["--map", "lockout", "--transport", "tcp"],
        ] {
            assert!(parsed(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn env_lines_are_quote_safe_for_eval() {
        let lease = json!({"id": "hl-x1", "map": "lockout", "variant": "it's"});
        let lines = env_lines("http://127.0.0.1:8074", &lease, "abc.def");
        assert_eq!(
            lines,
            "H2_LEASE=hl-x1\n\
             H2_HOST_LEASE_JSON='{\"id\":\"hl-x1\",\"map\":\"lockout\",\"variant\":\"it'\\''s\"}'\n\
             H2_TOKEN=abc.def\n\
             H2_LIVE=http://127.0.0.1:8074\n"
        );
        // The JSON round-trips through the quoting: strip the outer quotes
        // and undo the escape, and it parses to the same lease.
        let quoted = lines
            .lines()
            .nth(1)
            .unwrap()
            .strip_prefix("H2_HOST_LEASE_JSON=")
            .unwrap();
        let inner = &quoted[1..quoted.len() - 1];
        assert_eq!(
            serde_json::from_str::<Value>(&inner.replace("'\\''", "'")).unwrap(),
            lease
        );
        // A URL a shell would split on is quoted too.
        assert_eq!(
            shell_value("http://127.0.0.1:8074/?lease=1&token=2"),
            "'http://127.0.0.1:8074/?lease=1&token=2'"
        );
        // A token stays bare, and an empty value never becomes a bare word.
        assert_eq!(shell_value("aB.c-d_e~"), "aB.c-d_e~");
        assert_eq!(shell_value(""), "''");
    }
}
