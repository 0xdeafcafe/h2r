//! Link previews for the web app's pages (web/README.md, "Names, icons
//! and link previews"): the `<title>`, description, Open Graph and Twitter card
//! tags that Discord, iMessage and X read when a page is shared.
//!
//! The app renders in the browser and crawlers run no script, so the
//! server writes these into `index.html` for each route: [`describe`]
//! says what a path is (from the read model, as an anonymous reader may
//! see it), [`inject`] puts the tags in place of the block between
//! [`BEGIN`] and [`END`] in the built `index.html`. Both homes do it: the
//! machine's server in its SPA fallback, the Worker for the paths
//! wrangler.toml sends it first (it asks the hub for [`PageMeta`] at
//! `GET /v1/page-meta?path=` only when the page names a player, game or
//! clan).
//!
//! The product names come from the built HTML itself
//! (`<meta name="h2-dash-name">` and `<meta name="h2-stats-name">`,
//! which web/vite.config.ts fills in from web/src/brand.ts), so they are
//! set in one place.

use std::fmt::Write as _;

use live_proto::{Gamertag, Xuid};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Live;
use crate::sessions::Viewer;

/// The comment that opens the replaceable block in `index.html`.
pub const BEGIN: &str = "<!-- page-meta -->";
/// The comment that closes it.
pub const END: &str = "<!-- /page-meta -->";

/// The default card, in web/public (`bun run brand` in web/ draws it).
pub const DEFAULT_IMAGE: &str = "/og/default.jpg";
pub const DEFAULT_IMAGE_SIZE: (u32, u32) = (1200, 630);

/// The browser's colour on each side: the dashboard's dark green and the
/// stats site's navy (web/src/styles/theme.css).
pub const DASH_THEME: &str = "#0c4406";
pub const STATS_THEME: &str = "#071a35";

/// Names used when the HTML carries none.
pub const DEFAULT_DASH_NAME: &str = "Live Dashboard";
pub const DEFAULT_STATS_NAME: &str = "h2 Uplink";

/// Where the stats site lives: web/src/routes.ts's `STATS_ROOT`. It
/// leaves `/games/` for each game's site (Halo 2's now, Halo 1's later).
pub const STATS_ROOT: &str = "/games/halo2";

/// Where the stats site lived first. Its pages answer a permanent
/// redirect to the same page under [`STATS_ROOT`] ([`moved`]) and are
/// described as nothing else.
pub const OLD_STATS_ROOT: &str = "/stats";

/// The longest description written (crawlers cut at about 200).
const DESCRIPTION_MAX: usize = 200;

/// Which half of the site a page is on: its colour and the name it goes by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Dash,
    Stats,
}

/// What a page is, before it is written as tags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageMeta {
    /// The page's own title ("afr", "Slayer on Lockout"); none for a
    /// side's home, which goes by the side's name alone.
    pub page: Option<String>,
    pub description: String,
    pub side: Side,
    /// `profile` for a player, else `website`.
    #[serde(default = "website")]
    pub kind: String,
    /// The game page's pre-rendered header ([`crate::page_header`]), when
    /// the page names a game the reader may see: what the servers inject
    /// into the app's HTML beside these tags. Never in the tags themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<crate::page_header::GameHeaderModel>,
}

fn website() -> String {
    "website".into()
}

/// The two product names, as the built HTML gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub dash: String,
    pub stats: String,
}

impl Default for Names {
    fn default() -> Self {
        Names {
            dash: DEFAULT_DASH_NAME.into(),
            stats: DEFAULT_STATS_NAME.into(),
        }
    }
}

impl Names {
    /// The names `html` declares, else the defaults.
    pub fn from_html(html: &str) -> Names {
        let d = Names::default();
        Names {
            dash: meta_content(html, "h2-dash-name").unwrap_or(d.dash),
            stats: meta_content(html, "h2-stats-name").unwrap_or(d.stats),
        }
    }

    fn of(&self, side: Side) -> &str {
        match side {
            Side::Dash => &self.dash,
            Side::Stats => &self.stats,
        }
    }
}

/// The `content` of `<meta name="{name}" content="...">` in `html`, if it
/// is there and filled in (not an unreplaced `%PLACEHOLDER%`).
fn meta_content(html: &str, name: &str) -> Option<String> {
    let at = html.find(&format!("name=\"{name}\""))?;
    let start = html[..at].rfind('<')?;
    let end = at + html[at..].find('>')?;
    let tag = &html[start..end];
    let v = tag.split_once("content=\"")?.1.split_once('"')?.0.trim();
    (!v.is_empty() && !v.starts_with('%')).then(|| unescape(v))
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The path's segments, percent-decoded; empty ones dropped.
fn segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The page under [`STATS_ROOT`] that `path` (with its `query`) named
/// when the stats site was at [`OLD_STATS_ROOT`]; `None` for any other
/// path. Both homes answer it with a 301.
pub fn moved(path: &str, query: Option<&str>) -> Option<String> {
    let rest = path.strip_prefix(OLD_STATS_ROOT)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let rest = if rest == "/" { "" } else { rest };
    let mut to = format!("{STATS_ROOT}{rest}");
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        to.push('?');
        to.push_str(q);
    }
    Some(to)
}

/// The segments of a stats-site page after [`STATS_ROOT`]'s, or `None`
/// when `s` is not under it.
fn stats_page<'a>(s: &'a [&'a str]) -> Option<&'a [&'a str]> {
    match s {
        ["games", "halo2", rest @ ..] => Some(rest),
        _ => None,
    }
}

/// What a page names that only the read model can describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Player(String),
    Game(String),
    Clan(String),
}

/// The player, game or clan `path` is about, if any: the pages whose
/// preview needs the read model. Everything else [`describe`]s without it
/// (so the Worker need not wake the hub).
pub fn subject(path: &str) -> Option<Subject> {
    let s = segments(path);
    let s: Vec<&str> = s.iter().map(String::as_str).collect();
    if let Some(page) = stats_page(&s) {
        return match page {
            ["players", tag, ..] => Some(Subject::Player(tag.to_string())),
            ["games", id, ..] => Some(Subject::Game(id.to_string())),
            ["clans", id, ..] | ["teams", id, ..] => Some(Subject::Clan(id.to_string())),
            _ => None,
        };
    }
    // The oldest links, which the app redirects into the stats site.
    match s.as_slice() {
        ["players", tag, ..] => Some(Subject::Player(tag.to_string())),
        ["games", id, ..] => Some(Subject::Game(id.to_string())),
        ["dash", "clans", id, ..] => Some(Subject::Clan(id.to_string())),
        _ => None,
    }
}

/// Whether `path` is one of the app's pages (as opposed to the API, art
/// or a file): what the Worker answers with the app's HTML, or for a
/// page under [`OLD_STATS_ROOT`], its redirect ([`moved`]).
pub fn is_page(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    matches!(
        first,
        "" | "dash" | "stats" | "players" | "games" | "events" | "signin"
    )
}

/// The page at `path` without the read model: its section's title, or
/// for a page with a [`subject`], a plain one.
pub fn describe_static(path: &str) -> PageMeta {
    let s = segments(path);
    let s: Vec<&str> = s.iter().map(String::as_str).collect();
    let dash = |page: Option<&str>, description: &str| PageMeta {
        page: page.map(str::to_string),
        description: description.into(),
        side: Side::Dash,
        kind: website(),
        header: None,
    };
    let stats = |page: Option<&str>, description: &str| PageMeta {
        page: page.map(str::to_string),
        description: description.into(),
        side: Side::Stats,
        kind: website(),
        header: None,
    };
    const DASH: &str = "The dashboard of a community online service for h2recomp: who is online, games running now, friends, clans and matchmaking.";
    const STATS: &str = "Live games, leaderboards, service records and carnage reports from a community online service for h2recomp.";
    if let Some(page) = stats_page(&s) {
        return match page {
            [] => stats(None, STATS),
            ["games"] => stats(
                Some("Games"),
                "Games being played now and the latest finished, with their carnage reports.",
            ),
            ["games", ..] => stats(Some("Game"), STATS),
            ["players"] => stats(Some("Players"), "Who is online and the leaderboards."),
            ["players", ..] => stats(Some("Service Record"), STATS),
            ["ranks", ..] => stats(Some("Ranked"), "Ranked playlists and their leaderboards."),
            ["clans"] | ["teams"] => stats(Some("Clans"), "Every clan, its members and its games."),
            ["clans", ..] | ["teams", ..] => stats(Some("Clan"), STATS),
            ["events"] => stats(Some("Event Log"), "The service's signed public event log."),
            ["admin"] => stats(Some("Admin"), STATS),
            _ => stats(None, STATS),
        };
    }
    match s.as_slice() {
        [] | ["dash"] => dash(None, DASH),
        ["signin"] => stats(Some("Sign in"), STATS),
        ["dash", "signin"] => dash(Some("Sign in"), DASH),
        ["dash", "signup"] => dash(Some("Create an account"), DASH),
        ["dash", "clans", ..] => dash(Some("Clans"), DASH),
        ["dash", "players", ..] | ["players"] => dash(Some("Players"), DASH),
        ["dash", section, ..] => dash(Some(&title_case(section)), DASH),
        // `/games` redirects to the stats home; the oldest links into it.
        ["games"] => stats(None, STATS),
        ["games", ..] => stats(Some("Game"), STATS),
        ["players", ..] => stats(Some("Service Record"), STATS),
        ["events"] => stats(Some("Event Log"), "The service's signed public event log."),
        _ => dash(None, DASH),
    }
}

fn title_case(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The page at `path` as an anonymous reader sees it: [`describe_static`],
/// with the player, game or clan it names filled in from the read model,
/// and a game page's pre-rendered header ([`crate::page_header`]) beside
/// it. A server whose reads need an account says nothing beyond the static.
pub fn describe(live: &Live, path: &str) -> PageMeta {
    let mut base = describe_static(path);
    if live.authorize_read(None).is_err() {
        return base;
    }
    let header = crate::page_header::model_for(live, path);
    match subject(path) {
        Some(Subject::Player(tag)) => base = player(live, &tag).unwrap_or(base),
        Some(Subject::Game(id)) => base = game(live, &id).unwrap_or(base),
        Some(Subject::Clan(id)) => base = clan(live, &id, base.side).unwrap_or(base),
        None => {}
    }
    base.header = header;
    base
}

fn player(live: &Live, tag: &str) -> Option<PageMeta> {
    let tag = Gamertag::new(tag).ok()?;
    let (xuid, name) = {
        let s = live.model.read();
        let a = s.account_by_tag(&tag)?;
        (a.xuid, a.gamertag.to_string())
    };
    let summary = live.stats.summary("all", crate::app::now(), false);
    let row = summary
        .as_deref()
        .and_then(|v| v["players"].as_array())
        .and_then(|rows| rows.iter().find(|r| r["xuid"] == xuid.to_string()));
    Some(PageMeta {
        page: Some(name.clone()),
        description: record_line(&name, row),
        side: Side::Stats,
        kind: "profile".into(),
        header: None,
    })
}

/// "afr: 120 games, 64 wins, 1,503 kills, 1.23 K/D." from a stats row.
fn record_line(name: &str, row: Option<&Value>) -> String {
    let n = |k: &str| row.and_then(|r| r[k].as_u64()).unwrap_or(0);
    let games = n("games");
    if games == 0 {
        return format!("{name}'s service record: no games played yet.");
    }
    let mut parts = vec![plural(games, "game"), plural(n("wins"), "win")];
    if n("multiplayer") > 0 {
        let (kills, deaths) = (n("kills"), n("deaths"));
        parts.push(plural(kills, "kill"));
        let kd = if deaths == 0 {
            kills as f64
        } else {
            kills as f64 / deaths as f64
        };
        parts.push(format!("{kd:.2} K/D"));
    }
    if n("legendary") > 0 {
        parts.push(format!("{} on Legendary", plural(n("legendary"), "level")));
    }
    format!("{name}'s service record: {}.", parts.join(", "))
}

fn plural(n: u64, word: &str) -> String {
    let s = if n == 1 { "" } else { "s" };
    format!("{} {word}{s}", thousands(n))
}

fn thousands(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

const TEAMS: [&str; 8] = [
    "Red", "Blue", "Green", "Gold", "Purple", "Pink", "Teal", "Brown",
];

fn team_name(team: i32) -> String {
    usize::try_from(team)
        .ok()
        .and_then(|t| TEAMS.get(t))
        .map_or_else(|| format!("Team {}", team + 1), |n| n.to_string())
}

fn game(live: &Live, id: &str) -> Option<PageMeta> {
    if !live_proto::game::valid_id(id) || live.check_game_shows(id, Viewer::Anonymous).is_err() {
        return None;
    }
    let (summary, scoreboard, winner) = {
        let s = live.model.read();
        let g = s.games.get(id)?;
        (
            g.summary(),
            g.scoreboard.clone(),
            g.ended.as_ref().and_then(|e| e.0.winner.clone()),
        )
    };
    let map = summary
        .map_id
        .as_ref()
        .and_then(|m| live.map(&m.to_string()).ok())
        .map(|a| a.map.summary.name)
        .unwrap_or_else(|| map_title(&summary.map));
    let campaign = summary.kind == "campaign";
    let what = if campaign {
        "Campaign".to_string()
    } else {
        summary
            .variant
            .clone()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| title_case(&summary.mode))
    };
    let page = format!("{what} on {map}");
    let mut parts: Vec<String> = Vec::new();
    parts.push(if summary.state == "live" {
        "Live now".into()
    } else {
        "Final".into()
    });
    if let Some(d) = summary.difficulty.as_deref().filter(|_| campaign) {
        parts.push(title_case(d));
    }
    let name_of = |index: u32| {
        summary
            .players
            .iter()
            .find(|p| p.index == index)
            .map(|p| p.name.clone())
    };
    if !campaign {
        match &summary.teams {
            Some(teams) if summary.team_play && !teams.is_empty() => {
                let mut t = teams.clone();
                t.sort_by(|a, b| b.score.cmp(&a.score).then(a.team.cmp(&b.team)));
                parts.push(
                    t.iter()
                        .map(|t| format!("{} {}", team_name(t.team), t.score))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            _ => {
                let mut rows = scoreboard.clone();
                rows.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
                let top: Vec<String> = rows
                    .iter()
                    .take(3)
                    .filter_map(|r| Some(format!("{} {}", name_of(r.index)?, r.score)))
                    .collect();
                if !top.is_empty() {
                    parts.push(top.join(", "));
                }
            }
        }
        match winner {
            Some(live_proto::game::Winner::Team { team }) => {
                parts.push(format!("{} team wins", team_name(team)))
            }
            Some(live_proto::game::Winner::Player { player }) => {
                if let Some(n) = name_of(player) {
                    parts.push(format!("{n} wins"));
                }
            }
            None => {}
        }
    }
    let names: Vec<&str> = summary
        .players
        .iter()
        .filter(|p| !p.is_bot())
        .map(|p| p.name.as_str())
        .collect();
    if !names.is_empty() {
        let shown = names.iter().take(6).copied().collect::<Vec<_>>().join(", ");
        let more = names.len().saturating_sub(6);
        parts.push(if more > 0 {
            format!("with {shown} and {more} more")
        } else {
            format!("with {shown}")
        });
    }
    Some(PageMeta {
        page: Some(page),
        description: format!("{}.", parts.join(" · ")),
        side: Side::Stats,
        kind: website(),
        header: None,
    })
}

/// A retail map's stem as a title ("burial_mounds" -> "Burial Mounds").
/// The real names are in the owner's game art, not here.
fn map_title(stem: &str) -> String {
    let stem = stem.rsplit(['/', '\\']).next().unwrap_or(stem);
    let stem = stem.strip_suffix(".map").unwrap_or(stem);
    title_case(stem)
}

fn clan(live: &Live, id: &str, side: Side) -> Option<PageMeta> {
    let id = Xuid::parse(id).ok()?;
    let s = live.model.read();
    let t = s.teams.get(id).ok()?;
    let p = &t.properties;
    let members = t.members.len() as u64;
    let mut description = format!("{}: {}.", p.name, plural(members, "member"));
    for extra in [&p.motto, &p.description] {
        let extra = extra.trim();
        if !extra.is_empty() {
            let _ = write!(description, " {extra}");
        }
    }
    Some(PageMeta {
        page: Some(p.name.clone()),
        description,
        side,
        kind: website(),
        header: None,
    })
}

/// [`PageMeta`] as tags, for `path` under `origin` (`https://host`, no
/// trailing slash).
pub fn render(meta: &PageMeta, names: &Names, origin: &str, path: &str) -> String {
    let site = names.of(meta.side);
    let title = match &meta.page {
        Some(page) => format!("{page} · {site}"),
        None => site.to_string(),
    };
    let og_title = meta.page.clone().unwrap_or_else(|| site.to_string());
    let description = clip(&meta.description, DESCRIPTION_MAX);
    let theme = match meta.side {
        Side::Dash => DASH_THEME,
        Side::Stats => STATS_THEME,
    };
    let url = format!("{origin}{}", clean_path(path));
    let image = format!("{origin}{DEFAULT_IMAGE}");
    let (w, h) = DEFAULT_IMAGE_SIZE;
    let e = escape;
    let mut out = String::new();
    let _ = write!(
        out,
        "<title>{}</title>\n    \
         <meta name=\"description\" content=\"{}\" />\n    \
         <meta name=\"theme-color\" content=\"{theme}\" />\n    \
         <meta property=\"og:site_name\" content=\"{}\" />\n    \
         <meta property=\"og:type\" content=\"{}\" />\n    \
         <meta property=\"og:title\" content=\"{}\" />\n    \
         <meta property=\"og:description\" content=\"{}\" />\n    \
         <meta property=\"og:url\" content=\"{}\" />\n    \
         <meta property=\"og:image\" content=\"{}\" />\n    \
         <meta property=\"og:image:width\" content=\"{w}\" />\n    \
         <meta property=\"og:image:height\" content=\"{h}\" />\n    \
         <meta property=\"og:image:alt\" content=\"{}\" />\n    \
         <meta name=\"twitter:card\" content=\"summary_large_image\" />\n    \
         <meta name=\"twitter:title\" content=\"{}\" />\n    \
         <meta name=\"twitter:description\" content=\"{}\" />\n    \
         <meta name=\"twitter:image\" content=\"{}\" />",
        e(&title),
        e(&description),
        e(site),
        e(&meta.kind),
        e(&og_title),
        e(&description),
        e(&url),
        e(&image),
        e(&format!(
            "{}: a chrome 2 over the Earth at sunrise",
            names.of(meta.side)
        )),
        e(&og_title),
        e(&description),
        e(&image),
    );
    out
}

/// The path as written into `og:url`: no query or fragment, and only
/// characters that need no escaping beyond the attribute's.
fn clean_path(path: &str) -> String {
    let p = path.split(['?', '#']).next().unwrap_or("/");
    if p.is_empty() { "/".into() } else { p.into() }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max - 1).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a);
    format!("{}…", cut.trim_end_matches([',', '.', ' ', '·']))
}

/// HTML attribute and text escaping.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `html` with `tags` in place of its [`BEGIN`]..[`END`] block; without
/// one, before `</head>`; without that, unchanged.
pub fn inject(html: &str, tags: &str) -> String {
    if let Some(b) = html.find(BEGIN)
        && let Some(e) = html[b..].find(END)
    {
        let e = b + e + END.len();
        return format!("{}{BEGIN}\n    {tags}\n    {END}{}", &html[..b], &html[e..]);
    }
    if let Some(h) = html.find("</head>") {
        return format!("{}  {tags}\n  {}", &html[..h], &html[h..]);
    }
    html.to_string()
}

/// The page's HTML with its link preview and, when supplied, game header.
pub fn page_html(index: &str, meta: &PageMeta, origin: &str, path: &str) -> String {
    let names = Names::from_html(index);
    let html = inject(index, &render(meta, &names, origin, path));
    match &meta.header {
        Some(header) => crate::page_header::inject(&html, header),
        None => html,
    }
}

/// Where the server is reached from outside, for absolute URLs: the
/// configured public URL, else the request's `Host` (https unless it
/// says http through `X-Forwarded-Proto` or is a loopback name).
pub fn origin(public: Option<&str>, host: Option<&str>, forwarded_proto: Option<&str>) -> String {
    if let Some(p) = public.map(str::trim).filter(|p| !p.is_empty()) {
        return p.trim_end_matches('/').to_string();
    }
    let host = host.map(str::trim).filter(|h| {
        !h.is_empty()
            && h.chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
    });
    let Some(host) = host else {
        return String::new();
    };
    let local = host.starts_with("localhost") || host.starts_with("127.") || host.starts_with('[');
    let scheme = match forwarded_proto.map(str::trim) {
        Some("http") => "http",
        Some("https") => "https",
        _ if local => "http",
        _ => "https",
    };
    format!("{scheme}://{host}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "<!doctype html>\n<html>\n  <head>\n    <meta charset=\"UTF-8\" />\n    \
        <!-- page-meta -->\n    <title>%H2_DASH_NAME%</title>\n    <!-- /page-meta -->\n    \
        <meta name=\"h2-dash-name\" content=\"Live Dashboard\" />\n    \
        <meta name=\"h2-stats-name\" content=\"h2 Uplink\" />\n  </head>\n  <body></body>\n</html>\n";

    #[test]
    fn names_come_from_the_html() {
        assert_eq!(
            Names::from_html(INDEX),
            Names {
                dash: "Live Dashboard".into(),
                stats: "h2 Uplink".into()
            }
        );
        // Unreplaced placeholders and missing tags fall back.
        let raw = "<meta name=\"h2-dash-name\" content=\"%H2_DASH_NAME%\">";
        assert_eq!(Names::from_html(raw), Names::default());
        assert_eq!(Names::from_html(""), Names::default());
    }

    #[test]
    fn subjects_are_the_pages_that_need_the_read_model() {
        assert_eq!(
            subject("/games/halo2/players/afr"),
            Some(Subject::Player("afr".into()))
        );
        assert_eq!(
            subject("/games/halo2/players/Two%20Words/career"),
            Some(Subject::Player("Two Words".into()))
        );
        assert_eq!(subject("/players/afr"), Some(Subject::Player("afr".into())));
        assert_eq!(
            subject("/games/halo2/games/abc/3d"),
            Some(Subject::Game("abc".into()))
        );
        assert_eq!(subject("/games/abc"), Some(Subject::Game("abc".into())));
        assert_eq!(
            subject("/games/halo2/clans/0009000000000001"),
            Some(Subject::Clan("0009000000000001".into()))
        );
        assert_eq!(
            subject("/games/halo2/teams/x"),
            Some(Subject::Clan("x".into()))
        );
        for none in [
            "/",
            "/dash",
            "/games",
            "/games/halo2",
            "/games/halo2/games",
            "/games/halo2/players",
            "/games/halo2/clans",
            "/stats/players/afr",
            "/v1/games/abc",
        ] {
            assert_eq!(subject(none), None, "{none}");
        }
    }

    #[test]
    fn pages_are_the_app_routes() {
        for page in [
            "/",
            "/dash",
            "/dash/players",
            "/games",
            "/games/halo2",
            "/games/halo2/games/x",
            "/stats",
            "/stats/games/x",
            "/players/x",
            "/signin",
        ] {
            assert!(is_page(page), "{page}");
        }
        for not in [
            "/v1/games",
            "/assets/x.js",
            "/art/game/x",
            "/favicon.ico",
            "/og/default.jpg",
            "/healthz",
        ] {
            assert!(!is_page(not), "{not}");
        }
    }

    #[test]
    fn static_pages_have_their_section_and_side() {
        let m = describe_static("/");
        assert_eq!((m.page, m.side), (None, Side::Dash));
        let m = describe_static("/dash/sessions");
        assert_eq!((m.page.as_deref(), m.side), (Some("Sessions"), Side::Dash));
        let m = describe_static("/games/halo2");
        assert_eq!((m.page, m.side), (None, Side::Stats));
        let m = describe_static("/games/halo2/ranks/team-slayer");
        assert_eq!((m.page.as_deref(), m.side), (Some("Ranked"), Side::Stats));
        let m = describe_static("/games/halo2/players/afr");
        assert_eq!(
            (m.page.as_deref(), m.kind.as_str()),
            (Some("Service Record"), "website")
        );
        let m = describe_static("/games/halo2/events");
        assert_eq!((m.page.as_deref(), m.side), (Some("Event Log"), Side::Stats));
        let m = describe_static("/games");
        assert_eq!((m.page, m.side), (None, Side::Stats));
    }

    #[test]
    fn the_old_stats_root_moves_under_games() {
        assert_eq!(segments(STATS_ROOT), ["games", "halo2"]);
        assert_eq!(moved("/stats", None).as_deref(), Some("/games/halo2"));
        assert_eq!(moved("/stats/", None).as_deref(), Some("/games/halo2"));
        assert_eq!(
            moved("/stats/players/Two%20Words/career", Some("compare=x")).as_deref(),
            Some("/games/halo2/players/Two%20Words/career?compare=x")
        );
        assert_eq!(
            moved("/stats", Some("window=30d")).as_deref(),
            Some("/games/halo2?window=30d")
        );
        assert_eq!(
            moved("/stats/games/abc", Some("")).as_deref(),
            Some("/games/halo2/games/abc")
        );
        for not in ["/", "/statsx", "/games/halo2", "/v1/stats/summary", "/dash/stats"] {
            assert_eq!(moved(not, None), None, "{not}");
        }
    }

    #[test]
    fn tags_name_the_page_and_its_side_and_are_escaped() {
        let names = Names {
            dash: "Live Dashboard".into(),
            stats: "h2 Uplink".into(),
        };
        let meta = PageMeta {
            page: Some("<b>\"x\" & y".into()),
            description: "one\ntwo".into(),
            side: Side::Stats,
            kind: "profile".into(),
            header: None,
        };
        let t = render(
            &meta,
            &names,
            "https://live.example",
            "/games/halo2/players/x?y=1#z",
        );
        assert!(
            t.contains("<title>&lt;b&gt;&quot;x&quot; &amp; y · h2 Uplink</title>"),
            "{t}"
        );
        assert!(t.contains("<meta property=\"og:site_name\" content=\"h2 Uplink\" />"));
        assert!(t.contains("<meta property=\"og:type\" content=\"profile\" />"));
        assert!(t.contains("<meta name=\"description\" content=\"one two\" />"));
        assert!(t.contains(
            "<meta property=\"og:url\" content=\"https://live.example/games/halo2/players/x\" />"
        ));
        assert!(t.contains(
            "<meta property=\"og:image\" content=\"https://live.example/og/default.jpg\" />"
        ));
        assert!(t.contains("<meta name=\"theme-color\" content=\"#071a35\" />"));
        assert!(t.contains("<meta name=\"twitter:card\" content=\"summary_large_image\" />"));
        // A side's home goes by the side's name.
        let home = render(&describe_static("/"), &names, "https://live.example", "/");
        assert!(home.contains("<title>Live Dashboard</title>"));
        assert!(home.contains("content=\"#0c4406\""));
    }

    #[test]
    fn injection_replaces_the_block_once_and_keeps_the_rest() {
        let out = inject(INDEX, "<title>T</title>");
        assert!(
            out.contains("<!-- page-meta -->\n    <title>T</title>\n    <!-- /page-meta -->"),
            "{out}"
        );
        assert!(!out.contains("%H2_DASH_NAME%"));
        assert!(out.contains("<meta name=\"h2-stats-name\""));
        assert_eq!(out.matches("<title>").count(), 1);
        // Injected again (a cached copy), still one block.
        let twice = inject(&out, "<title>U</title>");
        assert_eq!(twice.matches("<title>").count(), 1);
        // No block: before </head>; no head: as it was.
        assert!(inject("<head></head>", "<x>").starts_with("<head>  <x>"));
        assert_eq!(
            inject("<!doctype html><title>x</title>", "<x>"),
            "<!doctype html><title>x</title>"
        );
    }

    #[test]
    fn descriptions_are_clipped_on_a_word() {
        let long = "word ".repeat(80);
        let c = clip(&long, 50);
        assert!(c.chars().count() <= 50 && c.ends_with("word…"), "{c}");
    }

    #[test]
    fn records_read_as_a_line() {
        let row = serde_json::json!({"games": 1203, "wins": 640, "multiplayer": 1100, "kills": 15030, "deaths": 12219, "legendary": 1});
        assert_eq!(
            record_line("afr", Some(&row)),
            "afr's service record: 1,203 games, 640 wins, 15,030 kills, 1.23 K/D, 1 level on Legendary."
        );
        assert_eq!(
            record_line("new", None),
            "new's service record: no games played yet."
        );
    }

    #[test]
    fn origins_prefer_the_public_url() {
        assert_eq!(
            origin(Some("https://live.example/"), Some("x"), None),
            "https://live.example"
        );
        assert_eq!(
            origin(None, Some("live.example"), None),
            "https://live.example"
        );
        assert_eq!(
            origin(None, Some("live.example"), Some("http")),
            "http://live.example"
        );
        assert_eq!(
            origin(None, Some("127.0.0.1:8075"), None),
            "http://127.0.0.1:8075"
        );
        assert_eq!(origin(None, Some("evil\"><script>"), None), "");
        assert_eq!(origin(None, None, None), "");
    }

    #[test]
    fn map_stems_read_as_titles() {
        assert_eq!(map_title("burial_mounds"), "Burial Mounds");
        assert_eq!(map_title("maps/lockout.map"), "Lockout");
    }
}
