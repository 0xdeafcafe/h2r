//! `h2 live q <what>`: read the service, for people and for agents
//! (docs/live.md, "For agents"). JSON when stdout is not a terminal (or
//! with `--json`), one JSON value per line with `--jsonl`, a compact table
//! on a terminal. Lists are fetched whole, following every cursor.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};
use live_proto::audit::{self, Chains, Link};
use live_proto::sign::PublicKey;
use live_proto::{AccountKeys, Author, StreamMessage, WireEvent, Xuid};
use serde_json::{Value, json};

use crate::live_cmd::{Args, Client, call, status_of};

const USAGE: &str = "usage: h2 live q <what> [--server URL] [--json|--jsonl]

  accounts                every account: [{xuid, gamertag, created_at, online}]
  account <gamertag>      one account, with keys' owner id and recent_games
  presence                who is signed in: [{xuid, gamertag, state, game_id?, since}]
  games [--live|--ended]  games, newest first: [{game_id, map, mode, variant,
                          team_play, state, started_at, ended_at, players, teams}]
  game <id> [--frames]    one game (scoreboard, last 50 kills, winner), or its
                          recorded frames: [{game_id, tick, players, source}]
  events [--from N] [--type T] [--stream S] [--follow]
                          the log after position N: [{position, stream, version,
                          type, at, data, author, kid, jws, jws_index?, hash}];
                          --follow then streams new ones as JSON lines
  keys <gamertag>         {keys: [{kid, jwk, valid_from, valid_to}], position}
  sessions                open session advertisements
  session <id>            one advertisement
  teams [gamertag]       open teams, or one account's teams and pending recruits
  team <id>              one team with members, properties and permissions
  friends <gamertag>      one account's friends, with presence, and pending
                          requests in and out: {friends: [{xuid, gamertag,
                          friends_since, presence?}], requests_in, requests_out}
  verify [--from N]       re-check every event's signature and hash chain here:
                          {checked, anchored, bad: [{position, stream, type,
                          problem}], position}; exits 1 if any is bad
  health                  {ok, position}

JSON when stdout is not a terminal; a table on one. --jsonl prints one value
per line (each item of a list).";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Json,
    Jsonl,
    Table,
}

/// What one query produced: a document, a list, or a list to follow.
enum Output {
    One(Value),
    List(Vec<Value>),
}

pub fn run(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let format = match (args.flag("--json"), args.flag("--jsonl")) {
        (true, true) => bail!("--json or --jsonl, not both"),
        (true, false) => Format::Json,
        (false, true) => Format::Jsonl,
        (false, false) if std::io::stdout().is_terminal() => Format::Table,
        _ => Format::Json,
    };
    if args.0.is_empty() || args.0.iter().any(|a| a == "--help" || a == "-h") {
        bail!("{USAGE}");
    }
    let what = args.0.remove(0);
    let client = Client::connect(server)?;
    let out = std::io::stdout();
    let mut out = out.lock();
    match what.as_str() {
        "accounts" => {
            no_more(args)?;
            emit(
                &mut out,
                format,
                Output::List(pages(&client, "/accounts", &[])?),
                ACCOUNTS,
            )
        }
        "account" => {
            let tag = one(args, "account takes a gamertag")?;
            let detail: Value = client.get(&format!("/accounts/{}", path_part(&tag)), &[])?;
            emit(&mut out, format, Output::One(detail), &[])
        }
        "presence" => {
            no_more(args)?;
            emit(
                &mut out,
                format,
                Output::List(pages(&client, "/presence", &[])?),
                PRESENCE,
            )
        }
        "games" => {
            let state = match (args.flag("--live"), args.flag("--ended")) {
                (true, true) => bail!("--live or --ended, not both"),
                (true, false) => vec![("state", "live".to_string())],
                (false, true) => vec![("state", "ended".to_string())],
                _ => Vec::new(),
            };
            no_more(args)?;
            emit(
                &mut out,
                format,
                Output::List(pages(&client, "/games", &state)?),
                GAMES,
            )
        }
        "game" => {
            let frames = args.flag("--frames");
            let id = one(args, "game takes a game id")?;
            let id = path_part(&id);
            if frames {
                let list = pages(&client, &format!("/games/{id}/frames"), &[])?;
                emit(&mut out, format, Output::List(list), FRAMES)
            } else {
                let detail: Value = client.get(&format!("/games/{id}"), &[])?;
                emit(&mut out, format, Output::One(detail), &[])
            }
        }
        "events" => events(&client, args, format, &mut out),
        "keys" => {
            let tag = one(args, "keys takes a gamertag")?;
            let keys: Value = client.get(&format!("/accounts/{}/keys", path_part(&tag)), &[])?;
            emit(&mut out, format, Output::One(keys), &[])
        }
        "sessions" => {
            let state = args.take("--state")?;
            let hopper = args.take("--hopper")?;
            let map = args.take("--map")?;
            let host = args.take("--host")?;
            no_more(args)?;
            let mut query = Vec::new();
            query.extend(state.map(|v| ("state", v)));
            query.extend(hopper.map(|v| ("hopper", v)));
            query.extend(map.map(|v| ("map", v)));
            query.extend(host.map(|v| ("host", v)));
            emit(
                &mut out,
                format,
                Output::List(pages(&client, "/sessions", &query)?),
                SESSIONS,
            )
        }
        "session" => {
            let id = one(args, "session takes an id")?;
            let value: Value = client.get(&format!("/sessions/{}", path_part(&id)), &[])?;
            emit(&mut out, format, Output::One(value), &[])
        }
        "teams" => {
            let args = args.positional()?;
            let path = match args.as_slice() {
                [] => "/teams".to_string(),
                [tag] => format!("/accounts/{}/teams", path_part(tag)),
                _ => bail!("teams takes zero or one gamertag"),
            };
            emit(
                &mut out,
                format,
                Output::List(pages(&client, &path, &[("limit", "100".into())])?),
                TEAMS,
            )
        }
        "team" => {
            let id = one(args, "team takes an id")?;
            let value: Value = client.get(&format!("/teams/{}", path_part(&id)), &[])?;
            emit(&mut out, format, Output::One(value), &[])
        }
        "friends" => {
            let tag = one(args, "friends takes a gamertag")?;
            let friends: Value =
                client.get(&format!("/accounts/{}/friends", path_part(&tag)), &[])?;
            emit(&mut out, format, Output::One(friends), &[])
        }
        "verify" => {
            let from = from(&mut args)?;
            no_more(args)?;
            let report = verify(&client, from)?;
            let bad = report["bad"].as_array().is_some_and(|b| !b.is_empty());
            emit(&mut out, format, Output::One(report), &[])?;
            if bad {
                std::process::exit(1);
            }
            Ok(())
        }
        "health" => {
            no_more(args)?;
            let text = call(client.agent.get(&format!("{}/healthz", client.base)), None)?;
            emit(
                &mut out,
                format,
                Output::One(serde_json::from_str(&text)?),
                &[],
            )
        }
        other => bail!("unknown query {other:?}\n\n{USAGE}"),
    }
}

fn no_more(args: Args) -> Result<()> {
    if !args.positional_for(USAGE)?.is_empty() {
        bail!("too many arguments\n\n{USAGE}");
    }
    Ok(())
}

fn one(args: Args, what: &str) -> Result<String> {
    let [v] = args
        .positional_for(USAGE)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("{what}\n\n{USAGE}"))?;
    Ok(v)
}

fn from(args: &mut Args) -> Result<u64> {
    args.take("--from")?
        .map_or(Ok(0), |v| v.parse())
        .context("--from must be a log position")
}

/// A gamertag or id as one URL path segment (gamertags have spaces).
pub(crate) fn path_part(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Every item of a list endpoint, following `next`.
fn pages(client: &Client, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut q = query.to_vec();
        if !q.iter().any(|(key, _)| *key == "limit") {
            q.push(("limit", "1000".into()));
        }
        if let Some(c) = cursor.take() {
            q.push(("cursor", c));
        }
        let page: Value = client.get(path, &q)?;
        items.extend(page["items"].as_array().cloned().unwrap_or_default());
        match page["next"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return Ok(items),
        }
    }
}

fn events(client: &Client, mut args: Args, format: Format, out: &mut impl Write) -> Result<()> {
    let from = from(&mut args)?;
    let kind = args.take("--type")?;
    let stream = args.take("--stream")?;
    let follow = args.flag("--follow");
    no_more(args)?;
    let mut query = vec![("from", from.to_string())];
    query.extend(kind.iter().map(|t| ("type", t.clone())));
    query.extend(stream.iter().map(|s| ("stream", s.clone())));
    if !follow {
        return emit(
            out,
            format,
            Output::List(pages(client, "/events", &query)?),
            EVENTS,
        );
    }
    // Following: the stream from `from`, one event per line as it comes.
    let mut url = format!("{}/stream?", client.api.replacen("http", "ws", 1));
    for (k, v) in &query {
        url.push_str(&format!("{k}={}&", path_part(v)));
    }
    let (mut ws, _) = tungstenite::connect(url.trim_end_matches('&'))
        .with_context(|| format!("opening {url}"))?;
    let line = |out: &mut dyn Write, v: &Value| -> Result<()> {
        if format == Format::Table {
            writeln!(out, "{}", row(v, EVENTS).join("  "))?;
        } else {
            writeln!(out, "{v}")?;
        }
        Ok(out.flush()?)
    };
    loop {
        let msg = ws.read().context("reading the stream")?;
        let tungstenite::Message::Text(text) = msg else {
            if msg.is_close() {
                return Ok(());
            }
            continue;
        };
        match serde_json::from_str::<StreamMessage>(&text) {
            Ok(StreamMessage::Event { event }) => line(out, &serde_json::to_value(&event)?)?,
            Ok(_) => {}
            Err(e) => bail!("an unexpected stream message: {e}"),
        }
    }
}

/// Re-checks the log after `from`: each event's JWS against its fields and
/// the author's key at its position, and each stream's hash chain.
fn verify(client: &Client, from: u64) -> Result<Value> {
    let mut keys = Keys {
        client,
        accounts: HashMap::new(),
    };
    let mut chains = Chains::default();
    let (mut checked, mut anchored) = (0u64, 0u64);
    let mut bad = Vec::new();
    let mut position = from;
    loop {
        let page: Value = client.get(
            "/events",
            &[("from", position.to_string()), ("limit", "1000".into())],
        )?;
        let items: Vec<WireEvent> =
            serde_json::from_value(page["items"].clone()).context("an unexpected events page")?;
        for e in &items {
            position = e.position;
            let key = keys.at(&e.author, &e.kid, e.position)?;
            let problem =
                audit::check_signature(e, key.as_ref())
                    .err()
                    .or_else(|| match chains.check(e) {
                        Ok(Link::Checked) => None,
                        Ok(Link::Anchored) => {
                            anchored += 1;
                            None
                        }
                        Err(p) => Some(p),
                    });
            match problem {
                None => checked += 1,
                Some(p) => bad.push(json!({"position": e.position, "stream": e.stream, "type": e.kind, "problem": p.to_string()})),
            }
        }
        if page["next"].is_null() || items.is_empty() {
            return Ok(
                json!({"checked": checked, "anchored": anchored, "bad": bad, "position": position}),
            );
        }
    }
}

/// Authors' keys, as the server publishes them, fetched once per account.
struct Keys<'a> {
    client: &'a Client,
    accounts: HashMap<Xuid, Option<AccountKeys>>,
}

impl Keys<'_> {
    fn at(&mut self, author: &str, kid: &str, position: u64) -> Result<Option<PublicKey>> {
        let jwk = match Author::parse(author) {
            Some(Author::Server(id)) if id == self.client.discovery.id => self
                .client
                .discovery
                .keys
                .iter()
                .find(|k| k.kid.as_deref() == Some(kid))
                .cloned(),
            Some(Author::Account { xuid, server }) if server == self.client.discovery.id => {
                if !self.accounts.contains_key(&xuid) {
                    // An unknown account has no keys; a failed lookup is fatal.
                    let keys = match self.client.keys(xuid) {
                        Ok(k) => Some(k),
                        Err(e) if status_of(&e) == Some(404) => None,
                        Err(e) => return Err(e),
                    };
                    self.accounts.insert(xuid, keys);
                }
                self.accounts[&xuid].as_ref().and_then(|k| {
                    k.keys
                        .iter()
                        .find(|k| {
                            k.kid == kid
                                && k.valid_from <= position
                                && k.valid_to.is_none_or(|t| position < t)
                        })
                        .map(|k| k.jwk.clone())
                })
            }
            _ => None,
        };
        Ok(jwk.and_then(|j| PublicKey::from_jwk(&j).ok()))
    }
}

type Columns = &'static [(&'static str, &'static str)];

const ACCOUNTS: Columns = &[
    ("xuid", "/xuid"),
    ("gamertag", "/gamertag"),
    ("online", "/online"),
    ("created", "/created_at"),
];
const PRESENCE: Columns = &[
    ("gamertag", "/gamertag"),
    ("state", "/state"),
    ("game", "/game_id"),
    ("since", "/since"),
];
const GAMES: Columns = &[
    ("game", "/game_id"),
    ("state", "/state"),
    ("map", "/map"),
    ("variant", "/variant"),
    ("team", "/team_play"),
    ("players", "/players"),
    ("started", "/started_at"),
];
const FRAMES: Columns = &[
    ("tick", "/tick"),
    ("players", "/players"),
    ("source", "/source"),
];
const SESSIONS: Columns = &[
    ("id", "/session_id"),
    ("host", "/host_gamertag"),
    ("state", "/state"),
    ("address", "/endpoint/addr"),
    ("port", "/endpoint/port"),
    ("open", "/advertisement/slots/public_open"),
];
const TEAMS: Columns = &[
    ("id", "/id"),
    ("name", "/properties/name"),
    ("leader", "/creator"),
    ("limit", "/max_members"),
];
const EVENTS: Columns = &[
    ("pos", "/position"),
    ("type", "/type"),
    ("stream", "/stream"),
    ("author", "/author"),
    ("at", "/at"),
];

fn emit(out: &mut impl Write, format: Format, output: Output, columns: Columns) -> Result<()> {
    match (format, output) {
        (Format::Json, Output::One(v)) => writeln!(out, "{}", serde_json::to_string_pretty(&v)?)?,
        (Format::Json, Output::List(items)) => {
            writeln!(out, "{}", serde_json::to_string_pretty(&items)?)?
        }
        (Format::Jsonl, Output::One(v)) => writeln!(out, "{v}")?,
        (Format::Jsonl, Output::List(items)) => {
            for v in items {
                writeln!(out, "{v}")?;
            }
        }
        (Format::Table, Output::List(items)) => {
            let rows: Vec<Vec<String>> = items.iter().map(|v| row(v, columns)).collect();
            let headers: Vec<&str> = columns.iter().map(|(h, _)| *h).collect();
            out.write_all(table(&headers, &rows).as_bytes())?;
        }
        (Format::Table, Output::One(v)) => out.write_all(fields(&v).as_bytes())?,
    }
    Ok(())
}

/// A cell: strings bare, arrays by length, null as `-`.
fn cell(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => "-".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.len().to_string(),
        Some(v) => v.to_string(),
    }
}

fn row(v: &Value, columns: Columns) -> Vec<String> {
    columns
        .iter()
        .map(|(_, ptr)| cell(v.pointer(ptr)))
        .collect()
}

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return "(none)\n".into();
    }
    const MAX: usize = 40;
    let clip = |s: &str| {
        if s.chars().count() > MAX {
            format!("{}…", s.chars().take(MAX - 1).collect::<String>())
        } else {
            s.to_string()
        }
    };
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| clip(c)).collect())
        .collect();
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([h.len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        format!("{}\n", padded.join("  ").trim_end())
    };
    let mut s = line(headers.to_vec());
    for r in &rows {
        s += &line(r.iter().map(String::as_str).collect());
    }
    s
}

/// One object on a terminal: its scalar fields as `key: value`, then each
/// list of objects as a table of its scalar fields.
fn fields(v: &Value) -> String {
    let Value::Object(map) = v else {
        return format!("{v}\n");
    };
    let mut s = String::new();
    let mut lists = Vec::new();
    for (k, v) in map {
        match v {
            Value::Array(items) if items.iter().any(Value::is_object) => lists.push((k, items)),
            _ => s += &format!("{k}: {}\n", cell(Some(v))),
        }
    }
    for (k, items) in lists {
        let mut keys: Vec<&String> = Vec::new();
        for item in items {
            for (key, val) in item.as_object().into_iter().flatten() {
                if !val.is_object() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|i| keys.iter().map(|key| cell(i.get(key.as_str()))).collect())
            .collect();
        let headers: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        s += &format!("\n{k}:\n{}", table(&headers, &rows));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_query_follows_cursor_and_renders_endpoint_and_slots() {
        use live_proto::{lxm, sign::issue_token};
        let dir = std::env::temp_dir().join(format!(
            "h2-live-session-query-{}",
            live_proto::sign::random_id()
        ));
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            Default::default(),
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        let (account, key) = client.create_account("Host", None).unwrap();
        for port in [31000, 32000] {
            let token = issue_token(
                &key,
                &account.id,
                &client.discovery.id,
                lxm::SESSIONS_WRITE,
                crate::live_cmd::now(),
            );
            let req = client
                .authorized(
                    client
                        .agent
                        .post(&client.url("/sessions"))
                        .set("content-type", "application/json"),
                    token,
                )
                .unwrap();
            let data = json!({"addr":"127.0.0.1","port":port,"host_address":vec![0;36],"slots":{"public_filled":1,"public_open":7,"private_filled":0,"private_open":0},"attributes":{}});
            call(req, Some(serde_json::to_string(&data).unwrap().as_bytes())).unwrap();
        }
        let all = pages(&client, "/sessions", &[("limit", "1".into())]).unwrap();
        assert_eq!(all.len(), 2);
        let mut output = Vec::new();
        emit(
            &mut output,
            Format::Table,
            Output::List(all.clone()),
            SESSIONS,
        )
        .unwrap();
        let table = String::from_utf8(output).unwrap();
        assert!(
            table.contains("Host")
                && table.contains("127.0.0.1")
                && table.contains("31000")
                && table.contains("32000")
        );
        let mut output = Vec::new();
        emit(
            &mut output,
            Format::Json,
            Output::List(all.clone()),
            SESSIONS,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap(),
            Value::Array(all)
        );
        drop(server);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn teams_query_pages_and_table_include_the_published_clan() {
        use live_proto::{
            lxm,
            sign::{EventBody, issue_token},
            team::{Action, MemberProperties, Properties},
        };
        let dir = std::env::temp_dir().join(format!(
            "h2-live-team-query-{}",
            live_proto::sign::random_id()
        ));
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            Default::default(),
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        let (a, key) = client.create_account("Alpha", None).unwrap();
        for n in 1..=2 {
            let id = live_proto::Xuid(0x000a000000000000 + n);
            let action = Action::Create {
                id,
                properties: Properties {
                    name: format!("Clan{n}"),
                    ..Default::default()
                },
                member: MemberProperties::default(),
                max_members: 100,
            };
            let body = EventBody::at_unix(
                &key,
                &a.id,
                &format!("team-{id}"),
                action.kind(),
                serde_json::to_value(action).unwrap(),
                crate::live_cmd::now(),
            )
            .sign(&key);
            let token = issue_token(
                &key,
                &a.id,
                &client.discovery.id,
                lxm::EVENTS_WRITE,
                crate::live_cmd::now(),
            );
            let request = client
                .authorized(
                    client
                        .agent
                        .post(&client.url("/events"))
                        .set("content-type", "application/jose"),
                    token,
                )
                .unwrap();
            call(request, Some(body.as_bytes())).unwrap();
        }
        let all = pages(&client, "/teams", &[("limit", "1".into())]).unwrap();
        assert_eq!(all.len(), 2, "query follows the team ID cursor");
        assert_eq!(pages(&client, "/accounts/Alpha/teams", &[]).unwrap(), all);
        let mut output = Vec::new();
        emit(&mut output, Format::Table, Output::List(all.clone()), TEAMS).unwrap();
        let table = String::from_utf8(output).unwrap();
        assert!(table.contains("Clan1") && table.contains("Clan2"));
        assert!(table.contains("000a000000000001") && table.contains("leader"));
        let mut output = Vec::new();
        emit(&mut output, Format::Json, Output::List(all.clone()), TEAMS).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap(),
            Value::Array(all)
        );
        drop(server);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn queries_and_verify_against_a_server() {
        use live_proto::lxm;
        use live_proto::sign::{EventBody, issue_token, sign_batch};
        let dir = std::env::temp_dir().join(format!("h2-live-q-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let server = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            Default::default(),
        )
        .unwrap();
        let client = Client::at(&server.url).unwrap();
        let (a, key) = client.create_account("Alpha", None).unwrap();
        client.create_account("Bravo", None).unwrap();
        // A game: server-signed GameStarted, then a batch by the host.
        let token =
            |m: &str| issue_token(&key, &a.id, &client.discovery.id, m, crate::live_cmd::now());
        let game = json!({"map": "lockout", "mode": "multiplayer", "team_play": false,
            "players": [{"index": 0, "name": "Alpha", "xuid": a.xuid, "team": 0}, {"index": 1, "name": "Bot", "team": 1}]});
        let req = client
            .authorized(
                client.agent.post(&client.url("/games")),
                token(lxm::GAMES_WRITE),
            )
            .unwrap();
        let started: Value =
            serde_json::from_str(&call(req, Some(game.to_string().as_bytes())).unwrap()).unwrap();
        let id = started["game_id"].as_str().unwrap();
        let at = time::OffsetDateTime::now_utc();
        let bodies: Vec<EventBody> = [
            json!({"tick": 1, "victim": 1, "killer": 0, "kind": "kill"}),
            json!({"tick": 2, "victim": 0, "kind": "suicide"}),
        ]
        .into_iter()
        .map(|d| EventBody::new(&key, &a.id, &format!("game-{id}"), "PlayerKilled", d, at))
        .collect();
        let req = client
            .authorized(
                client
                    .agent
                    .post(&client.url(&format!("/games/{id}/events"))),
                token(lxm::GAMES_WRITE),
            )
            .unwrap();
        call(req, Some(sign_batch(&key, &bodies).as_bytes())).unwrap();

        assert_eq!(pages(&client, "/accounts", &[]).unwrap().len(), 2);
        let games = pages(&client, "/games", &[("state", "live".into())]).unwrap();
        assert_eq!(games[0]["game_id"], id);
        let report = verify(&client, 0).unwrap();
        assert_eq!(report["bad"], json!([]));
        assert_eq!(report["checked"], 9, "{report}");
        assert_eq!(report["anchored"], 0);
        // From the middle, streams already under way are anchored.
        let report = verify(&client, 5).unwrap();
        assert_eq!(
            (report["checked"].as_u64(), report["bad"].clone()),
            (Some(4), json!([])),
            "{report}"
        );
        assert_eq!(report["anchored"], 1);
        drop(server);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tables_line_up() {
        let rows = vec![
            vec!["0009".to_string(), "Pete".into()],
            vec!["000a".into(), "A Long Name".into()],
        ];
        let t = table(&["xuid", "gamertag"], &rows);
        assert_eq!(t, "xuid  gamertag\n0009  Pete\n000a  A Long Name\n");
        assert_eq!(table(&["a"], &[]), "(none)\n");
        let v = json!({"game_id": "g", "players": [1, 2], "variant": null, "team_play": true});
        assert_eq!(
            row(
                &v,
                &[
                    ("g", "/game_id"),
                    ("p", "/players"),
                    ("v", "/variant"),
                    ("t", "/team_play")
                ]
            ),
            ["g", "2", "-", "true"]
        );
    }

    #[test]
    fn objects_print_as_fields_and_tables() {
        let v = json!({"game_id": "g", "kills": [{"victim": 1, "pos": [0, 1, 2]}, {"victim": 2, "killer": 1}], "winner": {"team": 0}});
        assert_eq!(
            fields(&v),
            "game_id: g\nwinner: {\"team\":0}\n\nkills:\npos  victim  killer\n3    1       -\n-    2       1\n"
        );
    }

    #[test]
    fn output_formats() {
        let list = || Output::List(vec![json!({"a": 1}), json!({"a": 2})]);
        let mut out = Vec::new();
        emit(&mut out, Format::Jsonl, list(), &[]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "{\"a\":1}\n{\"a\":2}\n");
        let mut out = Vec::new();
        emit(&mut out, Format::Json, list(), &[]).unwrap();
        let back: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(back, json!([{"a": 1}, {"a": 2}]));
        assert_eq!(path_part("Sneaky Pete"), "Sneaky%20Pete");
    }
}
