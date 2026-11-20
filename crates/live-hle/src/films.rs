//! Fetch the signed-in account's Theater library off the game thread.

use std::io::{Read, Write};

use live_proto::{Author, Page, Xuid, game, lxm, sign};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{LiveConfig, account_setup, config::Account};

const MAX_LIST: u64 = 2 << 20;
const MAX_FILM: u64 = 64 << 20;

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
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default)]
    pub players: Vec<game::Player>,
    #[serde(default)]
    pub scoreboard: Vec<game::PlayerScore>,
}

#[derive(Debug, Deserialize)]
pub struct ArchiveClip {
    pub id: String,
    pub title: String,
    pub from_tick: u64,
    pub to_tick: u64,
}

#[derive(Debug, Deserialize)]
pub struct ClipWithFilm {
    pub clip: ArchiveClip,
    pub film: RecentFilm,
}

pub struct Archive {
    agent: ureq::Agent,
    api: String,
    server_id: String,
    account: Account,
}

impl Archive {
    pub fn connect(config: &LiveConfig, xuid: Option<Xuid>) -> Result<Option<Self>, String> {
        let Some(server) = config.server_url.as_deref().filter(|_| config.enabled) else {
            return Ok(None);
        };
        let accounts = config.accounts().map_err(|e| e.to_string())?;
        let account = accounts
            .into_iter()
            .find(|account| xuid == Some(Xuid(account.xuid)) && account.key.is_some())
            .ok_or("Sign in to Live to see account films")?;
        let (api, server_id) = account_setup::discover(server).map_err(|e| e.to_string())?;
        let agent = ureq::AgentBuilder::new()
            .timeout(account_setup::TIMEOUT)
            .redirects(0)
            .build();
        Ok(Some(Self {
            agent,
            api,
            server_id,
            account,
        }))
    }

    fn xuid(&self) -> Xuid {
        Xuid(self.account.xuid)
    }

    fn request(&self, method: &str, url: &str, permission: &str) -> ureq::Request {
        let issuer = Author::Account {
            xuid: self.xuid(),
            server: self.server_id.clone(),
        };
        let token = sign::issue_token(
            self.account.key.as_deref().expect("signed account"),
            &issuer.to_string(),
            &self.server_id,
            permission,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        self.agent
            .request(method, url)
            .set("Authorization", &format!("Bearer {token}"))
    }

    pub fn recent(&self) -> Result<Vec<RecentFilm>, String> {
        self.list("recent")
    }

    pub fn saved(&self) -> Result<Vec<RecentFilm>, String> {
        self.list("saved")
    }

    pub fn clips(&self) -> Result<Vec<ClipWithFilm>, String> {
        self.list("clips")
    }

    fn list<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>, String> {
        // A screen can display up to forty films. Read additional pages
        // so a filter can still find an older pinned film.
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let mut url = format!("{}/films?player={}&kind={kind}", self.api, self.xuid());
            if kind != "recent" {
                url.push_str("&limit=100");
                if let Some(after) = after {
                    url.push_str(&format!("&cursor={after}"));
                }
            }
            let page = read_list::<T>(self.request("GET", &url, lxm::READ))?;
            rows.extend(page.items);
            let next = page.next.and_then(|cursor| cursor.parse::<u64>().ok());
            match next {
                Some(next) if after.is_none_or(|previous| previous > next) => {
                    after = Some(next);
                }
                _ => return Ok(rows),
            }
        }
    }

    pub fn set_saved(&self, game: &str, saved: bool) -> Result<(), String> {
        check_id(game)?;
        let url = format!("{}/films/{game}/save", self.api);
        self.request(
            if saved { "POST" } else { "DELETE" },
            &url,
            lxm::FILMS_WRITE,
        )
        .call()
        .map(|_| ())
        .map_err(http_error)
    }

    pub fn delete_clip(&self, clip: &str) -> Result<(), String> {
        check_id(clip)?;
        let url = format!("{}/clips/{clip}", self.api);
        self.request("DELETE", &url, lxm::FILMS_WRITE)
            .call()
            .map(|_| ())
            .map_err(http_error)
    }

    pub fn download(&self, game: &str, mut out: impl Write) -> Result<(), String> {
        check_id(game)?;
        let url = format!("{}/games/{game}/film", self.api);
        let response = self
            .request("GET", &url, lxm::READ)
            .timeout(std::time::Duration::from_secs(60))
            .call()
            .map_err(http_error)?;
        let len = std::io::copy(&mut response.into_reader().take(MAX_FILM + 1), &mut out)
            .map_err(|e| format!("Could not download host film: {e}"))?;
        if len == 0 || len > MAX_FILM {
            return Err("Invalid host film size".into());
        }
        Ok(())
    }
}

fn check_id(id: &str) -> Result<(), String> {
    if !game::valid_id(id) {
        return Err("Invalid Live film ID".into());
    }
    Ok(())
}

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("Live films: HTTP {code}"),
        ureq::Error::Transport(_) => "Could not reach Live films".into(),
    }
}

fn read_list<T: DeserializeOwned>(request: ureq::Request) -> Result<Page<T>, String> {
    let response = request.call().map_err(http_error)?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_LIST + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Could not read Live films".to_owned())?;
    if body.len() as u64 > MAX_LIST {
        return Err("Live films response is too large".into());
    }
    serde_json::from_slice(&body).map_err(|_| "Invalid Live films response".to_owned())
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
        let request = ureq::get(&format!("{api}/films?player={}&kind=recent", Xuid(2)))
            .set("Authorization", "Bearer read-token");
        let rows = read_list::<RecentFilm>(request).unwrap().items;
        server.join().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].game_id, "archived-game");
        assert_eq!(rows[0].map, "h1_bloodgulch");
    }
}
