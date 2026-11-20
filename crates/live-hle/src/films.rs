//! Read the Live account's recent spectator archives off the game thread.

use std::io::Read;

use live_proto::{Author, Page, Xuid, lxm, sign};
use serde::Deserialize;

use crate::{LiveConfig, account_setup};

const MAX_REPLY: u64 = 25 * (live_proto::film::MAX_SUMMARY_BYTES as u64 + 4096);

#[derive(Debug, Deserialize)]
pub struct RecentFilm {
    pub game_id: String,
    pub map: String,
    pub mode: String,
    pub variant: Option<String>,
    pub started_at: String,
    pub duration_s: Option<u64>,
    pub result: Option<String>,
    #[serde(default)]
    pub has_host_film: bool,
    pub summary: Option<live_proto::film::Summary>,
}

/// Read the account handed to the game by the launcher, as telemetry does.
/// The public archive listing also applies the account's game visibility.
pub fn recent(config: &LiveConfig) -> Result<Vec<RecentFilm>, String> {
    let Some(server) = config.server_url.as_deref().filter(|_| config.enabled) else {
        return Ok(Vec::new());
    };
    let accounts = config.accounts().map_err(|e| e.to_string())?;
    let (account, key) = accounts
        .iter()
        .find_map(|account| account.key.as_deref().map(|key| (account, key)))
        .ok_or("Sign in to Live to see recent films")?;
    let (api, server_id) = account_setup::discover(server).map_err(|e| e.to_string())?;
    let xuid = Xuid(account.xuid);
    let issuer = Author::Account {
        xuid,
        server: server_id.clone(),
    };
    let bearer = sign::issue_token(
        key,
        &issuer.to_string(),
        &server_id,
        lxm::READ,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    );
    read_recent(&api, xuid, &bearer)
}

fn read_recent(api: &str, xuid: Xuid, token: &str) -> Result<Vec<RecentFilm>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(account_setup::TIMEOUT)
        .redirects(0)
        .build();
    let response = agent
        .get(&format!("{api}/films?player={xuid}&kind=recent"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(code, _) => format!("Live films: HTTP {code}"),
            ureq::Error::Transport(_) => "Could not reach Live films".into(),
        })?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_REPLY + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Could not read Live films".to_owned())?;
    if body.len() as u64 > MAX_REPLY {
        return Err("Live films response is too large".into());
    }
    let page: Page<RecentFilm> =
        serde_json::from_slice(&body).map_err(|_| "Invalid Live films response".to_owned())?;
    Ok(page.items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    #[test]
    fn recent_uses_the_accounts_archive_even_without_a_host_film() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut request = BufReader::new(&conn);
            let mut first = String::new();
            request.read_line(&mut first).unwrap();
            assert_eq!(
                first.trim_end(),
                "GET /v1/films?player=0000000000000002&kind=recent HTTP/1.1"
            );
            let mut authorised = false;
            loop {
                let mut line = String::new();
                let bytes = request.read_line(&mut line).unwrap();
                if bytes == 0 || line.trim_end().is_empty() {
                    break;
                }
                authorised |= line
                    .trim_end()
                    .eq_ignore_ascii_case("Authorization: Bearer read-token");
            }
            assert!(authorised);
            let body = serde_json::json!({
                "items": [{
                    "game_id": "archived-game", "map": "h1_bloodgulch",
                    "mode": "multiplayer", "variant": "Slayer",
                    "started_at": "2026-11-20T02:26:06.627Z", "duration_s": 69,
                    "result": "win", "has_host_film": false
                }],
                "next": null, "position": 1
            })
            .to_string();
            write!(
                conn,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let rows = read_recent(&api, Xuid(2), "read-token").unwrap();
        server.join().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].game_id, "archived-game");
        assert_eq!(rows[0].map, "h1_bloodgulch");
        assert!(!rows[0].has_host_film);
        assert!(rows[0].summary.is_none());
    }
}
