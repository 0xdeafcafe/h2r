//! The game page's pre-rendered header (web/src/stats/gameHeader.ts, the
//! web side's half and the source of the fixtures this file is held to).
//!
//! Both homes write the game header into the initial HTML. Each player's
//! image points directly at the composed emblem route ([`crate::emblem`]).
//!
//! [`model_for`] builds the header's model from the read model, as an
//! anonymous reader sees it; [`render`] writes its markup; [`json`] is the
//! model as the page carries it beside the header, for the client to
//! adopt what is already there (it re-renders only when the game has
//! actually moved on); [`inject`] puts the whole block in place of the
//! `game-header` comments in the built `index.html`. The client's
//! derivation and rendering are the TypeScript module above; the fixtures
//! under web/test/fixtures (made by its generator) are the contract both
//! sides are tested against, so their markup cannot drift apart.

use std::fmt::Write as _;

use live_proto::game::{self, GameEnded, GameSummary, Player, PlayerScore, Winner};
use serde::{Deserialize, Serialize};

use crate::app::Live;
use crate::page_meta;
use crate::sessions::Viewer;

/// The comment that opens the replaceable block in `index.html`.
pub const BEGIN: &str = "<!-- game-header -->";
/// The comment that closes it.
pub const END: &str = "<!-- /game-header -->";

/// The header's node and the model beside it, as the client finds them.
const HEADER_ID: &str = "game-header";
const MODEL_ID: &str = "game-header-model";

// ---- the model (the field order is the JSON's, and the client's) ----

/// One player in the header: their name, their emblem, their standing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPlayer {
    pub name: String,
    /// What the emblem is picked from: the XUID when known, else the name.
    pub seed: String,
    /// The art pack emblem (0..23), picked stably from the seed.
    pub emblem: u32,
    /// The composed image. Older page metadata only carried the fallback.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "emblemSrc")]
    pub emblem_src: Option<String>,
    /// This player's colour, `#rrggbb`.
    pub colour: String,
    /// A bot (a Clanker): marked beside the name. Left out when not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<bool>,
    /// Their score. Left out when the game has none (a campaign).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<i64>,
}

/// One team in the header: its colour, its score, its players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderTeam {
    pub team: i32,
    pub name: String,
    pub colour: String,
    pub score: i64,
    pub players: Vec<HeaderPlayer>,
}

/// What the header shows. The same JSON the client derives and renders
/// (web/src/stats/gameHeader.ts `GameHeaderModel`): plain data, in this
/// field order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameHeaderModel {
    pub game: String,
    pub live: bool,
    /// The variant's name ("Slayer"), or the map for a campaign.
    pub title: String,
    /// The map's display name.
    pub map: String,
    /// `LIVE` or `FINAL`.
    pub status: String,
    /// How it ended ("Red Team won"); null while it is live.
    pub result: Option<String>,
    /// The kind of game ("Custom Game, Team Game", "Campaign Co-op · Heroic").
    #[serde(rename = "type")]
    pub kind_line: String,
    /// Team play: the teams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<Vec<HeaderTeam>>,
    /// Free-for-all and campaign: the players, in standing order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<Vec<HeaderPlayer>>,
    /// A campaign game: the title is the mission, not "on" a map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<bool>,
}

// ---- constants mirrored from the web side ----

/// The art pack's emblems, by index (web/src/lib/art.ts `EMBLEMS`).
const PACK_EMBLEMS: [&str; 24] = [
    "01-double-arch", "02-pebble-stack", "03-folded-ribbon", "04-nested-waves", "05-notched-bars", "06-rising-hooks",
    "07-maze", "08-stepping-stones", "09-crescent-dot", "10-doorways", "11-three-fins", "12-pebble-orbit",
    "13-pillars", "14-open-link", "15-stepped-bridge", "16-paired-drops", "17-half-disc-stack", "18-capsule-dots",
    "19-curved-fan", "20-wave-bands", "21-roof-stack", "22-folded-loop", "23-river", "24-offset-orbits",
];

/// A stable fallback for a seed (web/src/lib/art.ts).
fn pack_emblem(seed: &str) -> u32 {
    crate::emblem::fnv(seed, 0) % PACK_EMBLEMS.len() as u32
}

fn emblem_img(player: &HeaderPlayer) -> String {
    let name = PACK_EMBLEMS[(player.emblem % PACK_EMBLEMS.len() as u32) as usize];
    let one = format!("/art/emblems/{name}.webp");
    let image = if let Some(src) = &player.emblem_src {
        let src = page_meta::escape(src);
        format!(r#"<img class="emblem emblem--game" src="{src}" data-emblem-fallback="{one}""#)
    } else {
        let two = format!("/art/emblems/{name}@2x.webp");
        format!(r#"<img class="emblem" src="{one}" srcset="{one} 1x, {two} 2x""#)
    };
    format!(r#"{image} width="16" height="16" alt="" decoding="async" draggable="false">"#)
}

/// Team colours, by team index (web/src/map/palette.ts `FALLBACK.teams`,
/// which the page's CSS never overrides: `--map-team-<i>` is never set).
const TEAM_COLOURS: [&str; 8] = [
    "#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#9b8a7a",
];
/// Free-for-all colours, by player index (palette.ts `FALLBACK.ffa`).
const FFA_COLOURS: [&str; 16] = [
    "#ff6f61", "#4aa3ff", "#3cc48f", "#ffb627", "#a07cff", "#ff8fc7", "#5ad1e0", "#c98b5c",
    "#e85d9b", "#6b8cff", "#8bd450", "#ff9152", "#7a6cff", "#40c4aa", "#d4a24a", "#b06cd9",
];

fn team_colour(team: i32) -> &'static str {
    let n = TEAM_COLOURS.len() as i32;
    TEAM_COLOURS[(team.rem_euclid(n)) as usize]
}

fn ffa_colour(index: u32) -> &'static str {
    FFA_COLOURS[(index % FFA_COLOURS.len() as u32) as usize]
}

/// The eight team names (web/src/lib/game.ts `teamName`).
fn team_name(team: i32) -> String {
    match usize::try_from(team).ok().and_then(|t| TEAMS.get(t)) {
        Some(name) => (*name).into(),
        None => format!("Team {}", team + 1),
    }
}

const TEAMS: [&str; 8] = [
    "Red", "Blue", "Green", "Gold", "Purple", "Pink", "Teal", "Brown",
];

/// A string title-cased as the web side does it (lib/format.ts
/// `titleCase`): `_-` runs fold to one space, and every word character
/// that begins a word is upper-cased.
fn title_case(s: &str) -> String {
    let mut spaced = String::with_capacity(s.len());
    let mut after_sep = false;
    for c in s.chars() {
        if c == '_' || c == '-' {
            if !after_sep {
                spaced.push(' ');
            }
            after_sep = true;
        } else {
            spaced.push(c);
            after_sep = false;
        }
    }
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(spaced.len());
    let mut prev_word = false;
    for c in spaced.chars() {
        if word(c) && !prev_word {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        prev_word = word(c);
    }
    out
}

/// A map file's stem as a title (the web side's header does the same:
/// the path and `.map` off, trimmed, title-cased).
fn map_title(map: &str) -> String {
    let stem = map.rsplit(['/', '\\']).next().unwrap_or(map);
    let stem = stem.strip_suffix(".map").unwrap_or(stem);
    title_case(stem.trim())
}

/// A variant's name for display (state/gametype.ts `variantName`): one
/// that already reads as words is kept, the rest are title-cased.
fn variant_name(v: Option<&str>) -> String {
    let t = v.unwrap_or_default().trim();
    if t.is_empty() {
        return String::new();
    }
    if t.chars().any(|c| c.is_ascii_uppercase() || c == ' ') {
        t.into()
    } else {
        title_case(t)
    }
}

// ---- building the model from the read model ----

/// The header's model for the game `path` names, as an anonymous reader
/// sees it; none for any other page, or a game they may not see.
pub fn model_for(live: &Live, path: &str) -> Option<GameHeaderModel> {
    if live.authorize_read(None).is_err() {
        return None;
    }
    let id = match page_meta::subject(path)? {
        page_meta::Subject::Game(id) => id,
        _ => return None,
    };
    if !game::valid_id(&id) || live.check_game_shows(&id, Viewer::Anonymous).is_err() {
        return None;
    }
    let s = live.model.read();
    let g = s.games.get(&id)?;
    let ended = g.ended.as_ref().map(|e| &e.0);
    Some(build(&g.summary(), &g.scoreboard, ended))
}

/// A player's score from the game's scoreboard, when it has one.
fn score_of(scoreboard: &[PlayerScore], index: u32) -> Option<i64> {
    scoreboard
        .iter()
        .find(|s| s.index == index)
        .map(|s| s.score)
}

fn build(
    summary: &GameSummary,
    scoreboard: &[PlayerScore],
    ended: Option<&GameEnded>,
) -> GameHeaderModel {
    let campaign = summary.mode == "campaign";
    let map = map_title(&summary.map);
    let live = summary.state == "live";
    // The players in standing order (score, then index), each with its
    // team for the fold below.
    let mut rows: Vec<&Player> = summary.players.iter().collect();
    rows.sort_by_key(|p| std::cmp::Reverse((score_of(scoreboard, p.index).unwrap_or(0), std::cmp::Reverse(p.index))));
    let header_player = |p: &Player| {
        let seed = p
            .xuid
            .map(|x| x.to_string())
            .unwrap_or_else(|| p.name.clone());
        HeaderPlayer {
            name: p.name.clone(),
            emblem: pack_emblem(&seed),
            emblem_src: Some(crate::emblem::image_path(&seed, p.appearance)),
            seed,
            colour: if summary.team_play {
                team_colour(p.team).into()
            } else {
                ffa_colour(p.index).into()
            },
            bot: p.is_bot().then_some(true),
            score: (!campaign).then(|| score_of(scoreboard, p.index)).flatten(),
        }
    };
    let players: Vec<HeaderPlayer> = rows.iter().copied().map(header_player).collect();
    let title = if campaign {
        map.clone()
    } else {
        let v = variant_name(summary.variant.as_deref());
        if v.is_empty() { title_case(&summary.mode) } else { v }
    };
    let base = GameHeaderModel {
        game: summary.game_id.clone(),
        live,
        title,
        map,
        status: if live { "LIVE" } else { "FINAL" }.into(),
        result: result_words(summary, ended),
        kind_line: type_line(summary),
        teams: None,
        players: None,
        campaign: campaign.then_some(true),
    };
    if campaign || !summary.team_play {
        return GameHeaderModel { players: Some(players), ..base };
    }
    // Team play: fold the standing players into their teams, then stand
    // the teams by score, then team number (the team's own score when the
    // game has it, else its players' added).
    let mut by: Vec<(i32, Vec<HeaderPlayer>)> = Vec::new();
    for (p, row) in players.into_iter().zip(rows.iter()) {
        match by.iter_mut().find(|(t, _)| *t == row.team) {
            Some((_, list)) => list.push(p),
            None => by.push((row.team, vec![p])),
        }
    }
    let team_score = |team: i32, ps: &[HeaderPlayer]| {
        summary
            .teams
            .as_ref()
            .and_then(|ts| ts.iter().find(|t| t.team == team))
            .map(|t| t.score)
            .unwrap_or_else(|| ps.iter().map(|p| p.score.unwrap_or(0)).sum())
    };
    let mut teams: Vec<HeaderTeam> = by
        .into_iter()
        .map(|(team, ps)| HeaderTeam {
            team,
            name: team_name(team),
            colour: team_colour(team).into(),
            score: team_score(team, &ps),
            players: ps,
        })
        .collect();
    teams.sort_by(|a, b| b.score.cmp(&a.score).then(a.team.cmp(&b.team)));
    GameHeaderModel { teams: Some(teams), ..base }
}

/// How a finished game ended (resultWords in web/src/stats/Side.tsx).
fn result_words(summary: &GameSummary, ended: Option<&GameEnded>) -> Option<String> {
    if summary.state != "ended" {
        return None;
    }
    let e = ended?;
    match &e.winner {
        Some(Winner::Team { team }) => Some(format!("{} Team won", team_name(*team))),
        Some(Winner::Player { player }) => {
            let name = summary
                .players
                .iter()
                .find(|p| p.index == *player)
                .map(|p| p.name.as_str())
                .unwrap_or("Someone");
            Some(format!("{name} placed 1st"))
        }
        None => Some(
            match e.reason.as_str() {
                "quit" => "Ended early",
                "lost" => "Unfinished",
                _ => "No winner",
            }
            .into(),
        ),
    }
}

/// The kind of game for the header's type line, as the client derives it
/// (playlist() in web/src/stats/Side.tsx).
fn type_line(summary: &GameSummary) -> String {
    if summary.mode == "campaign" {
        let label = if summary.online == Some(true) {
            "Campaign Online"
        } else if summary.players.len() > 1 {
            "Campaign Co-op"
        } else {
            "Campaign"
        };
        return match summary.difficulty.as_deref().map(|d| title_case(d)) {
            Some(d) => format!("{label} · {d}"),
            None => label.into(),
        };
    }
    match summary.kind.as_str() {
        "matchmaking" => match summary.playlist.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => format!("Matchmaking: {p}"),
            None => "Matchmaking".into(),
        },
        "clankermatch" => match summary.playlist.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => format!("Clankermatch: {p}"),
            None => "Clankermatch".into(),
        },
        _ => format!(
            "Custom Game, {}",
            if summary.team_play { "Team Game" } else { "Free for All" }
        ),
    }
}

// ---- rendering (the client renders these same bytes) ----

/// The header's markup (web/src/stats/gameHeader.ts `gameHeaderHtml`): one
/// line, no whitespace between tags, so both sides write the same bytes.
pub fn render(m: &GameHeaderModel) -> String {
    let e = page_meta::escape;
    let on = if m.campaign == Some(true) {
        String::new()
    } else {
        format!(" on {}", e(&m.map))
    };
    let flag = if m.live { " s-gamehead-flag--live" } else { "" };
    let result = m
        .result
        .as_ref()
        .map(|r| format!("<span>{}</span>", e(r)))
        .unwrap_or_default();
    let bot = r#"<i class="s-gamehead-bot">bot</i>"#;
    let player = |p: &HeaderPlayer| {
        let score = p
            .score
            .map(|s| format!(r#"<b class="s-gamehead-pscore">{s}</b>"#))
            .unwrap_or_default();
        format!(
            r#"<li class="s-gamehead-player" style="--row:{}">{}<span>{}</span>{}{}</li>"#,
            e(&p.colour),
            emblem_img(p),
            e(&p.name),
            if p.bot == Some(true) { bot } else { "" },
            score,
        )
    };
    let side = match &m.teams {
        Some(teams) => {
            let mut t = String::from(r#"<ul class="s-gamehead-teams">"#);
            for team in teams {
                let _ = write!(
                    t,
                    r#"<li class="s-gamehead-team" style="--row:{}"><header><span>{}</span><b>{}</b></header><ul class="s-gamehead-players">"#,
                    e(&team.colour),
                    e(&team.name),
                    team.score
                );
                for p in &team.players {
                    t.push_str(&player(p));
                }
                t.push_str("</ul></li>");
            }
            t.push_str("</ul>");
            t
        }
        None => {
            let mut t = String::from(r#"<ul class="s-gamehead-players s-gamehead-players--solo">"#);
            for p in m.players.iter().flatten() {
                t.push_str(&player(p));
            }
            t.push_str("</ul>");
            t
        }
    };
    format!(
        r#"<div class="s-gamehead" data-game="{}"><div class="s-gamehead-info"><p class="s-gamehead-title"><b>{}</b>{}</p><p class="s-gamehead-sub"><i class="s-gamehead-flag{}">{}</i>{}<small>{}</small></p></div><div class="s-gamehead-side">{}</div></div>"#,
        e(&m.game),
        e(&m.title),
        on,
        flag,
        e(&m.status),
        result,
        e(&m.kind_line),
        side,
    )
}

/// The model as the page carries it beside the header (a `<script
/// type="application/json">`): compact JSON, `<` escaped so it cannot
/// close the script, byte-identical to the client's `gameHeaderJson`.
pub fn json(m: &GameHeaderModel) -> String {
    serde_json::to_string(m)
        .expect("the model is plain data")
        .replace('<', "\\u003c")
}

/// `html` with the header for `m` in place of its [`BEGIN`]..[`END`]
/// block; without one, unchanged (an older build simply shows no header).
pub fn inject(html: &str, m: &GameHeaderModel) -> String {
    let Some(b) = html.find(BEGIN) else {
        return html.to_string();
    };
    let Some(e) = html[b..].find(END) else {
        return html.to_string();
    };
    let e = b + e + END.len();
    let block = format!(
        "{BEGIN}\n    <div id=\"{HEADER_ID}\" class=\"s-gamehead-shell\" data-game=\"{game}\">\n      <script type=\"application/json\" id=\"{MODEL_ID}\">{model}</script>\n      <div class=\"s-gamehead-col\">{inner}</div>\n    </div>\n    {END}",
        BEGIN = BEGIN,
        HEADER_ID = HEADER_ID,
        MODEL_ID = MODEL_ID,
        END = END,
        game = page_meta::escape(&m.game),
        model = json(m),
        inner = render(m),
    );
    format!("{}{block}{}", &html[..b], &html[e..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerIdentity, store::MemoryStore};
    use std::sync::Arc;

    /// The fixtures the web side's generator wrote (web/test/fixtures):
    /// the model a finished team game derives to, and the markup the
    /// client renders from it. This file must write the same bytes.
    const FIXTURE_JSON: &str = include_str!("../../../web/test/fixtures/game-header.json");
    const FIXTURE_HTML: &str = include_str!("../../../web/test/fixtures/game-header.html");

    #[test]
    fn the_fixture_model_renders_the_clients_markup() {
        let model: GameHeaderModel = serde_json::from_str(FIXTURE_JSON.trim()).unwrap();
        assert_eq!(render(&model), FIXTURE_HTML.trim());
    }

    #[test]
    fn the_model_serialises_as_the_client_does() {
        let model: GameHeaderModel = serde_json::from_str(FIXTURE_JSON.trim()).unwrap();
        // Field order and emptiness included: the client adopts the page's
        // header by comparing this string.
        assert_eq!(serde_json::to_string(&model).unwrap(), FIXTURE_JSON.trim());
    }

    #[test]
    fn the_fixture_model_derives_from_a_game_as_the_client_derives_it() {
        // The same game, as the read model would carry it (the web side's
        // fixture game, web/test/fixtures/game-header-game.ts).
        let summary = GameSummary {
            game_id: "0123abcd".into(),
            map: "orchard".into(),
            mode: "multiplayer".into(),
            kind: "custom".into(),
            variant: Some("Team Slayer".into()),
            playlist: None,
            playlist_id: None,
            difficulty: None,
            online: None,
            team_play: true,
            state: "ended".into(),
            started_at: "2026-11-18T20:00:00.000Z".into(),
            ended_at: Some("2026-11-18T20:11:30.000Z".into()),
            players: vec![
                Player {
                    index: 0,
                    name: "afr".into(),
                    xuid: Some(live_proto::Xuid::parse("0009000000000042").unwrap()),
                    team: 0,
                    local: Some(true),
                    appearance: Some(game::Appearance {
                        emblem_foreground: Some(30),
                        emblem_background: Some(7),
                        primary: Some(3),
                        secondary: Some(6),
                        tertiary: Some(8),
                        quaternary: Some(2),
                    }),
                    bot: None,
                },
                Player {
                    index: 1,
                    name: "Sneaky Pete".into(),
                    xuid: Some(live_proto::Xuid::parse("0009000000000043").unwrap()),
                    team: 1,
                    local: Some(false),
                    appearance: Some(game::Appearance {
                        primary: Some(7),
                        ..game::Appearance::default()
                    }),
                    bot: None,
                },
                Player {
                    index: 2,
                    name: "Clanker-7".into(),
                    xuid: None,
                    team: 1,
                    local: Some(false),
                    appearance: None,
                    bot: Some(true),
                },
                Player {
                    index: 3,
                    name: "Rock & Roll".into(),
                    xuid: None,
                    team: 0,
                    local: Some(false),
                    appearance: None,
                    bot: None,
                },
            ],
            teams: Some(vec![
                live_proto::game::TeamScore { team: 0, score: 18 },
                live_proto::game::TeamScore { team: 1, score: 20 },
            ]),
            map_id: None,
            skulls: None,
        };
        let scoreboard = vec![
            PlayerScore { index: 0, score: 15, kills: 14, deaths: 9, assists: 2 },
            PlayerScore { index: 1, score: 12, kills: 11, deaths: 10, assists: 1 },
            PlayerScore { index: 2, score: 8, kills: 8, deaths: 8, assists: 0 },
            PlayerScore { index: 3, score: 3, kills: 3, deaths: 12, assists: 0 },
        ];
        let ended = GameEnded {
            tick: 0,
            reason: "finished".into(),
            winner: Some(Winner::Team { team: 1 }),
            players: Vec::new(),
        };
        let built = build(&summary, &scoreboard, Some(&ended));
        let model: GameHeaderModel = serde_json::from_str(FIXTURE_JSON.trim()).unwrap();
        assert_eq!(serde_json::to_string(&built).unwrap(), serde_json::to_string(&model).unwrap());
    }

    #[test]
    fn injection_puts_the_header_above_the_app_once() {
        let model: GameHeaderModel = serde_json::from_str(FIXTURE_JSON.trim()).unwrap();
        let index = "<!doctype html>\n<body>\n  <!-- game-header -->\n  <!-- /game-header -->\n  <div id=\"root\"></div>\n</body>";
        let out = inject(index, &model);
        assert!(out.contains(r#"<div id="game-header" class="s-gamehead-shell" data-game="0123abcd">"#), "{out}");
        // The model beside it, as JSON that cannot close its script.
        assert!(out.contains(r#"<script type="application/json" id="game-header-model">"#), "{out}");
        assert!(out.contains("<b>Team Slayer</b> on Orchard"), "{out}");
        // The app's node still there, after the header.
        let root = out.find(r#"<div id="root">"#).unwrap();
        let head = out.find(r#"id="game-header""#).unwrap();
        assert!(head < root);
        // Once only, and injecting again keeps it once.
        assert_eq!(out.matches("s-gamehead\"").count(), 1);
        let twice = inject(&out, &model);
        assert_eq!(twice.matches("s-gamehead\"").count(), 1);
        // Without the block, unchanged.
        assert_eq!(inject("<body><div id=\"root\"></div></body>", &model), "<body><div id=\"root\"></div></body>");
    }

    #[test]
    fn every_emblem_element_has_fixed_dimensions() {
        let model: GameHeaderModel = serde_json::from_str(FIXTURE_JSON.trim()).unwrap();
        let html = render(&model);
        let imgs = html.match_indices("<img ").count();
        assert!(imgs > 0);
        for img in html.split("<img ").skip(1) {
            let tag = &img[..img.find('>').unwrap()];
            assert!(tag.contains(r#"width="16""#), "{tag}");
            assert!(tag.contains(r#"height="16""#), "{tag}");
        }
    }

    #[test]
    fn the_json_cannot_close_its_script_tag() {
        let model = GameHeaderModel {
            game: "</script><script>alert(1)</script>".into(),
            live: false,
            title: "T".into(),
            map: "M".into(),
            status: "FINAL".into(),
            result: None,
            kind_line: "Custom Game".into(),
            teams: None,
            players: Some(vec![HeaderPlayer {
                name: "</script>".into(),
                seed: "</script>".into(),
                emblem: 0,
                emblem_src: None,
                colour: "#ff6f61".into(),
                bot: None,
                score: None,
            }]),
            campaign: None,
        };
        let j = json(&model);
        assert!(!j.contains("</script>"));
        assert!(j.contains("\\u003c/script>"));
    }

    #[test]
    fn picks_and_titles_read_as_the_client_reads_them() {
        // The web side's test pins the same values.
        assert_eq!(pack_emblem("0009000000000042"), 16);
        assert_eq!(pack_emblem("Clanker-7"), 21);
        assert_eq!(title_case("orchard"), "Orchard");
        assert_eq!(title_case("01b_delta-halo"), "01b Delta Halo");
        assert_eq!(title_case("a--b"), "A B");
        assert_eq!(variant_name(None), "");
        assert_eq!(variant_name(Some("team slayer")), "Team Slayer");
        assert_eq!(variant_name(Some("Team Slayer")), "Team Slayer");
        assert_eq!(team_name(-1), "Team 0");
        assert_eq!(team_name(8), "Team 9");
    }

    #[tokio::test]
    async fn a_page_that_names_no_game_has_no_header() {
        let dir = std::env::temp_dir().join(format!(
            "h2-page-header-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = ServerIdentity::open(&dir, None).unwrap();
        let live = Live::start(Arc::new(MemoryStore::new()), identity, Default::default())
            .await
            .unwrap();
        for path in ["/", "/games/halo2", "/games/halo2/games"] {
            assert!(model_for(&live, path).is_none(), "{path}");
        }
        // A game the server has never seen.
        assert!(model_for(&live, "/games/halo2/games/0123abcd").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
