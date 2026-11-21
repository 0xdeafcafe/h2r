//! Fetch the signed-in account's Theater library off the game thread.
pub mod sync;

use std::io::{Read, Write};

use live_proto::{Author, Page, Xuid, game, lxm, sign};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{LiveConfig, account_setup, config::Account};

// Recent contains 25 entries. Keep the same page size for saved films and clips
// so a page of maximum-size summaries fits the bounded response reader.
const PAGE_SIZE: usize = 25;
const MAX_LIST: u64 = (2 << 20) + (PAGE_SIZE * live_proto::film::MAX_SUMMARY_BYTES) as u64;

#[derive(Debug, serde::Serialize, Deserialize)]
pub struct RecentFilm {
    pub game_id: String,
    #[serde(default)]
    pub saved: bool,
    #[serde(default)]
    pub shared: bool,
    #[serde(default)]
    pub visibility: Option<live_proto::film::sharing::Visibility>,
    #[serde(default)]
    pub length_ticks: u64,
    pub map: String,
    pub mode: String,
    pub variant: Option<String>,
    pub started_at: String,
    #[serde(default)]
    pub start_tick: Option<u64>,
    pub duration_s: Option<u64>,
    #[serde(default)]
    pub ticks_per_second: Option<u16>,
    pub result: Option<String>,
    #[serde(default)]
    pub has_host_film: bool,
    #[serde(default)]
    pub summary: Option<live_proto::film::Summary>,
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
            .timeout_connect(account_setup::TIMEOUT)
            .timeout_read(std::time::Duration::from_secs(60))
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

    pub fn library(&self) -> Result<Vec<RecentFilm>, String> {
        self.list("library")
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        check_id(id)?;
        match self
            .request(
                "DELETE",
                &format!("{}/films/{id}", self.api),
                lxm::FILMS_WRITE,
            )
            .call()
        {
            Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(http_error(e)),
        }
    }

    pub fn set_visibility(
        &self,
        id: &str,
        visibility: live_proto::film::sharing::Visibility,
    ) -> Result<(), String> {
        check_id(id)?;
        self.request(
            "PUT",
            &format!("{}/films/{id}/visibility", self.api),
            lxm::FILMS_WRITE,
        )
        .set("Content-Type", "application/json")
        .send_string(&serde_json::json!({"visibility":visibility}).to_string())
        .map(|_| ())
        .map_err(http_error)
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
        // Filters also need older pinned films and clips.
        let mut rows = Vec::new();
        let mut after = None;
        loop {
            let mut url = format!("{}/films?player={}&kind={kind}", self.api, self.xuid());
            if kind != "recent" {
                url.push_str(&format!("&limit={PAGE_SIZE}"));
                if let Some(after) = after {
                    url.push_str(&format!("&cursor={after}"));
                }
            }
            let page = read_list::<T>(self.request("GET", &url, lxm::READ))?;
            let next = page.next;
            rows.extend(page.items);
            let Some(next) = next else { return Ok(rows) };
            let next = next
                .parse::<u64>()
                .map_err(|_| "Invalid film list cursor")?;
            if kind == "recent" || after.is_some_and(|previous| previous <= next) {
                return Err("Invalid film list cursor".into());
            }
            after = Some(next);
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
        .timeout(account_setup::TIMEOUT)
        .call()
        .map(|_| ())
        .map_err(http_error)
    }

    pub fn share_clip(&self, game: &str, start: u64, end: u64) -> Result<String, String> {
        check_id(game)?;
        let url = format!("{}/films/{game}/clips", self.api);
        let response = self
            .request("POST", &url, lxm::FILMS_WRITE)
            .set("Content-Type", "application/json")
            .send_string(
                &serde_json::json!({"from_tick":start,"to_tick":end,"title":"Film clip"})
                    .to_string(),
            )
            .map_err(http_error)?;
        let clip: ArchiveClip = serde_json::from_reader(response.into_reader().take(4096))
            .map_err(|e| e.to_string())?;
        check_id(&clip.id)?;
        Ok(clip.id)
    }

    pub fn delete_clip(&self, clip: &str) -> Result<(), String> {
        check_id(clip)?;
        let url = format!("{}/clips/{clip}", self.api);
        self.request("DELETE", &url, lxm::FILMS_WRITE)
            .timeout(account_setup::TIMEOUT)
            .call()
            .map(|_| ())
            .map_err(http_error)
    }

    pub fn detail(&self, game: &str) -> Result<RecentFilm, String> {
        check_id(game)?;
        self.read_one(&format!("{}/films/{game}", self.api))
    }

    pub fn clip(&self, id: &str) -> Result<(String, ArchiveClip), String> {
        #[derive(Deserialize)]
        struct Row {
            game: String,
            #[serde(flatten)]
            clip: ArchiveClip,
        }
        check_id(id)?;
        let row: Row = self.read_one(&format!("{}/clips/{id}", self.api))?;
        Ok((row.game, row.clip))
    }

    fn read_one<T: DeserializeOwned>(&self, url: &str) -> Result<T, String> {
        let response = self
            .request("GET", url, lxm::READ)
            .timeout(account_setup::TIMEOUT)
            .call()
            .map_err(http_error)?;
        serde_json::from_reader(response.into_reader().take(MAX_LIST + 1))
            .map_err(|e| e.to_string())
    }

    fn exact(&self, game: &str) -> Result<ureq::Request, String> {
        check_id(game)?;
        Ok(self.request(
            "GET",
            &format!("{}/films/{game}/film?object=film", self.api),
            lxm::READ,
        ))
    }

    /// A one-byte authenticated probe also checks visibility on cache hits.
    pub fn revision(&self, game: &str) -> Result<Option<(u64, String)>, String> {
        let response = match self.exact(game)?.set("Range", "bytes=0-0").call() {
            Ok(r) => r,
            Err(ureq::Error::Status(409, _)) => return Ok(None),
            Err(e) => return Err(http_error(e)),
        };
        let size = response
            .header("Content-Range")
            .and_then(|h| h.strip_prefix("bytes 0-0/"))
            .and_then(|s| s.parse().ok())
            .ok_or("Invalid film size")?;
        let hash = response
            .header("ETag")
            .and_then(|h| h.strip_prefix("\"film-"))
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("Missing film digest")?;
        if response.status() != 206 {
            return Err("Live did not return a film range".into());
        }
        Ok(Some((size, hash.into())))
    }

    pub fn range(
        &self,
        game: &str,
        range: std::ops::Range<u64>,
        size: u64,
        digest: &str,
    ) -> Result<Vec<u8>, String> {
        let span = format!("{}-{}", range.start, range.end - 1);
        let etag = format!("\"film-{digest}\"");
        let response = self
            .exact(game)?
            .set("Range", &format!("bytes={span}"))
            .set("If-Match", &etag)
            .call()
            .map_err(http_error)?;
        if response.status() != 206
            || response.header("ETag") != Some(etag.as_str())
            || response.header("Content-Range") != Some(format!("bytes {span}/{size}").as_str())
        {
            return Err("Live returned a different film range".into());
        }
        let mut bytes = Vec::new();
        copy_film(
            response.into_reader(),
            &mut bytes,
            Some(range.end - range.start),
            range.end - range.start,
        )?;
        Ok(bytes)
    }

    pub fn download(&self, game: &str, mut out: impl Write) -> Result<(), String> {
        check_id(game)?;
        let url = format!("{}/games/{game}/film", self.api);
        let response = self
            .request("GET", &url, lxm::READ)
            .call()
            .map_err(http_error)?;
        if response.status() != 200 || response.header("Content-Range").is_some() {
            return Err("Live returned a partial host film".into());
        }
        let expected = response
            .header("Content-Length")
            .map(str::parse::<u64>)
            .transpose()
            .map_err(|_| "Invalid host film size")?;
        copy_film(
            response.into_reader(),
            &mut out,
            expected,
            live_proto::film::MAX_UPLOAD_BYTES,
        )
    }
}

fn copy_film(
    input: impl Read,
    out: &mut impl Write,
    expected: Option<u64>,
    limit: u64,
) -> Result<(), String> {
    if expected.is_some_and(|n| n == 0 || n > limit) {
        return Err("Invalid host film size".into());
    }
    let size = std::io::copy(&mut input.take(limit + 1), out)
        .map_err(|e| format!("Could not download host film: {e}"))?;
    if size == 0 || size > limit || expected.is_some_and(|n| n != size) {
        return Err("Invalid host film size".into());
    }
    Ok(())
}

fn check_id(id: &str) -> Result<(), String> {
    if !game::valid_id(id) {
        return Err("Invalid Live film ID".into());
    }
    Ok(())
}

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body =
                serde_json::from_reader::<_, serde_json::Value>(response.into_reader().take(4096))
                    .ok();
            body.as_ref()
                .and_then(|v| {
                    v["message"]
                        .as_str()
                        .or_else(|| v.pointer("/error/message")?.as_str())
                })
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Live films: HTTP {code}"))
        }
        ureq::Error::Transport(_) => "Could not reach Live films".into(),
    }
}

fn read_list<T: DeserializeOwned>(request: ureq::Request) -> Result<Page<T>, String> {
    let response = request
        .timeout(account_setup::TIMEOUT)
        .call()
        .map_err(http_error)?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_LIST + 1)
        .read_to_end(&mut body)
        .map_err(|_| "Could not read Live films".to_owned())?;
    if body.len() as u64 > MAX_LIST {
        return Err("Live films response is too large".into());
    }
    let page: Page<T> =
        serde_json::from_slice(&body).map_err(|_| "Invalid Live films response".to_owned())?;
    if page.items.len() > PAGE_SIZE {
        return Err("Too many films in Live response".into());
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };

    #[test]
    fn summary_and_game_listing_can_coexist() {
        let clip: ClipWithFilm = serde_json::from_value(serde_json::json!({
            "clip": {"id": "clip", "title": "Finish", "from_tick": 120, "to_tick": 160},
            "film": {
                "game_id": "recorded-game", "map": "lockout", "mode": "multiplayer",
                "started_at": "2026-11-20T02:26:06Z", "kind": "matchmaking",
                "has_host_film": true, "ticks_per_second": 60,
                "players": [{"index": 0, "name": "Guest", "team": 1}],
                "scoreboard": [{"index": 0, "score": 4, "kills": 4, "deaths": 1, "assists": 0}],
                "summary": {"level": "lockout", "game_type": "Slayer", "mode": "matchmaking",
                    "variant_name": "Team Slayer", "duration_ticks": 162, "players": []}
            }
        }))
        .unwrap();
        assert!(clip.film.has_host_film);
        assert_eq!(clip.film.ticks_per_second, Some(60));
        assert_eq!(clip.film.players[0].name, "Guest");
        assert_eq!(clip.film.scoreboard[0].score, 4);
        assert_eq!(clip.film.summary.unwrap().duration_ticks, 162);
    }

    #[test]
    fn full_download_checks_declared_and_streamed_lengths() {
        let bytes = b"recording".as_slice();
        let len = bytes.len() as u64;
        for declared in [None, Some(len)] {
            let mut out = Vec::new();
            copy_film(bytes, &mut out, declared, len).unwrap();
            assert_eq!(out, bytes);
        }
        let mut out = Vec::new();
        assert!(copy_film(bytes, &mut out, Some(len + 1), len).is_err());
        assert!(out.is_empty());
        assert!(copy_film(bytes, &mut out, None, len - 1).is_err());
        assert!(copy_film(&bytes[1..], &mut out, Some(len), len).is_err());
        assert!(copy_film(&bytes[..0], &mut out, None, len).is_err());
    }

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
        assert!(!rows[0].has_host_film);
        assert!(rows[0].summary.is_none());
    }
}
