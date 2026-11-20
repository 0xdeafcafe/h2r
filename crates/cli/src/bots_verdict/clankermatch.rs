//! The verdict of a Clankermatch run (`h2 bots test --clankermatch`,
//! docs/game/bots.md §8 and §13, "Clankermatch"): the Xbox Live menu's
//! Clankermatch row, a clanker playlist, the HLE's empty search, the lobby
//! this console hosts filled by phantoms (so it reaches the
//! playlist's fewest players and starts), the game record, and the bots
//! going once the player leaves.
//!
//! The merged log is the custom game test's ([`super`]), with
//! `bots-test: clankermatch plan: ...` in place of its plan line, the
//! server's marks as `bots-test: server clanker hoppers [..]` and the game
//! record as [`game_line`].

use super::{
    Check, Log, Plan, Verdict, check, phantom_failed, phantom_joined, phantom_joined_lobby,
    phantom_off, read_status,
};

const PLAN: &str = "bots-test: clankermatch plan: ";
/// The harness's line for the server's marks (`GET /v1/hoppers/clankers`).
pub const SERVER_MARKS: &str = "bots-test: server clanker hoppers ";
/// The HLE's line for the marks it fetched at logon.
const GAME_MARKS: &str = "live: clankermatch: clanker hoppers ";
const ACTIVE: &str = "clankermatch: the playlists are the clanker hoppers";
const EMPTY_LIST: &str = "clankermatch: the server named no clanker hoppers";
const SEARCH: &str = "live: clankermatch: searching clanker playlist ";
const HUMAN_SEARCH: &str = "live: optimatch: searching playlist ";
const HOSTING: &str = "clankers: clankermatch: hosting clanker playlist ";
const FILL_PREFIX: &str = "clankers: clankermatch: ";
const BOTS_GO: [&str; 2] = [
    "clankers: clankermatch: the game is over: the bots go",
    "clankers: clankermatch: this console hosts another game: the bots go",
];

/// What a Clankermatch run asked for: its `bots-test: clankermatch plan:`
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmPlan {
    /// `slayer`, `teams` or `doubles` (`--playlist`).
    pub playlist: String,
    /// Its title (`Clanker Slayer`).
    pub title: String,
    /// The hopper (descriptor index) it is, as searches and sessions name
    /// it.
    pub hopper: u32,
    /// Its row on the Clankermatch playlist list.
    pub row: u32,
    /// The fewest players a game starts with, and the most.
    pub min: u32,
    pub max: u32,
    /// Teams (0: free for all).
    pub teams: u32,
    /// Bots beside the human, every one a phantom.
    pub bots: usize,
    pub tag: String,
}

impl CmPlan {
    pub fn phantoms(&self) -> usize {
        self.bots
    }

    /// `bots-test: clankermatch plan: playlist slayer (Clanker Slayer),
    /// hopper 8, row 0, players 2 to 8, teams 0, bots 7 (7 phantoms),
    /// gamertag Tester`
    pub fn line(&self) -> String {
        format!(
            "{PLAN}playlist {} ({}), hopper {}, row {}, players {} to {}, teams {}, bots {} \
             ({} phantoms), gamertag {}",
            self.playlist,
            self.title,
            self.hopper,
            self.row,
            self.min,
            self.max,
            self.teams,
            self.bots,
            self.phantoms(),
            self.tag
        )
    }

    /// The plan a log's plan line names.
    pub fn read(texts: &[&str]) -> Option<CmPlan> {
        let rest = texts.iter().find_map(|t| t.strip_prefix(PLAN))?;
        let (playlist, rest) = rest.strip_prefix("playlist ")?.split_once(" (")?;
        let (title, rest) = rest.split_once("), hopper ")?;
        let (hopper, rest) = rest.split_once(", row ")?;
        let (row, rest) = rest.split_once(", players ")?;
        let (min, rest) = rest.split_once(" to ")?;
        let (max, rest) = rest.split_once(", teams ")?;
        let (teams, rest) = rest.split_once(", bots ")?;
        let (bots, rest) = rest.split_once(" (")?;
        let (_, tag) = rest.split_once(" phantoms), gamertag ")?;
        Some(CmPlan {
            playlist: playlist.into(),
            title: title.into(),
            hopper: hopper.parse().ok()?,
            row: row.parse().ok()?,
            min: min.parse().ok()?,
            max: max.parse().ok()?,
            teams: teams.parse().ok()?,
            bots: bots.parse().ok()?,
            tag: tag.trim().into(),
        })
    }
}

/// The game's own lobby count: `bots: lobby players: N in the game's
/// session` (`bots::lobby::players_line`).
fn read_lobby_players(text: &str) -> Option<u64> {
    let rest = text.strip_prefix("bots: lobby players: ")?;
    rest.split(' ').next()?.parse().ok()
}

/// A status line of a game starting: a multiplayer map loading or in
/// play, not the main menu's (`map mainmenu`, mode `main_menu`) nor the
/// campaign's.
fn game_start(text: &str) -> bool {
    read_status(text).is_some_and(|(phase, mode, map)| {
        matches!(phase, "loading" | "gameplay")
            && map != "mainmenu"
            && mode != "main_menu"
            && mode != "campaign"
    })
}

/// `[8, 9, 10]`: the hoppers.
pub fn read_marks(text: &str) -> Option<Vec<u32>> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(|n| n.trim().parse().ok()).collect()
}

/// `bots-test: game ID on MAP: P players, B bots (A with an account), kind
/// K, playlist NAME` (`-` for none).
pub fn game_line(
    id: &str,
    map: &str,
    (players, bots, accounts): (usize, usize, usize),
    kind: &str,
    playlist: &str,
) -> String {
    format!(
        "{}, kind {kind}, playlist {playlist}",
        super::game_line(id, map, players, bots, accounts)
    )
}

/// A [`game_line`]'s bots, kind and playlist.
fn read_game(text: &str) -> Option<(usize, &str, &str)> {
    let (bots, _) = super::read_game(text)?;
    let (_, rest) = text.rsplit_once(", kind ")?;
    let (kind, playlist) = rest.split_once(", playlist ")?;
    Some((bots, kind, playlist))
}

/// `live: session ID advertised ...`: the id.
fn advertised_id(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("live: session ")?;
    let (id, after) = rest.split_once(' ')?;
    after.starts_with("advertised").then_some(id)
}

/// `clankers: clankermatch: hosting clanker playlist H (session S): ...`:
/// the hopper and session.
fn hosting(text: &str) -> Option<(u32, &str)> {
    let rest = text.strip_prefix(HOSTING)?;
    let (h, rest) = rest.split_once(" (session ")?;
    let (s, _) = rest.split_once(')')?;
    Some((h.parse().ok()?, s))
}

/// `clankers: clankermatch: N phantoms join the lobby ...`: N.
fn phantoms_planned(text: &str) -> Option<usize> {
    let rest = text.strip_prefix(FILL_PREFIX)?;
    let (n, after) = rest.split_once(' ')?;
    after
        .starts_with("phantoms join the lobby")
        .then(|| n.parse().ok())?
}

impl Log<'_> {
    /// The first line at or after `from` whose text `f` keeps: its index.
    fn index_from(&self, from: usize, f: impl Fn(&str) -> bool) -> Option<usize> {
        (from..self.texts.len()).find(|&i| f(self.texts[i]))
    }

    fn raw_at(&self, i: usize) -> String {
        self.raw[i].to_string()
    }
}

fn drive_lines(l: &Log) -> Vec<String> {
    let mut lines = l.last(4, |t| t.starts_with("bots-drive"));
    lines.extend(l.last(1, |t| t.starts_with("ui: screen")));
    lines
}

/// The verdict of a Clankermatch log (`plan` its plan line).
pub fn verdict(l: &Log, plan: CmPlan) -> Verdict {
    let mut checks: Vec<Check> = Vec::new();

    // 1. Signed in to Live.
    checks.push(match l.first(|t| t.starts_with("live: playing as ")) {
        Some((line, t)) => check(
            "signed in to Xbox Live",
            true,
            t["live: playing as ".len()..].to_string(),
            vec![line],
        ),
        None => check(
            "signed in to Xbox Live",
            false,
            format!(
                "the game never said `live: playing as {}`: it did not sign in (or never booted; \
                 see the game's full log)",
                plan.tag
            ),
            drive_lines(l),
        ),
    });

    // 2. The clanker hoppers: the server's marks, and the game's copy.
    let server = l.first(|t| t.starts_with(SERVER_MARKS));
    let game = l.first(|t| t.starts_with(GAME_MARKS));
    let has = |t: &str, prefix: &str| {
        t.strip_prefix(prefix)
            .and_then(read_marks)
            .is_some_and(|m| m.contains(&plan.hopper))
    };
    checks.push(match (&server, &game) {
        (Some((sl, st)), Some((gl, gt))) if has(*st, SERVER_MARKS) && has(*gt, GAME_MARKS) => {
            check(
                "clanker hoppers listed (server and game)",
                true,
                format!(
                    "the server and the game mark {}; {} is hopper {}",
                    &gt[GAME_MARKS.len()..],
                    plan.title,
                    plan.hopper
                ),
                vec![sl.clone(), gl.clone()],
            )
        }
        _ => {
            let mut lines: Vec<String> = server.iter().map(|s| s.0.clone()).collect();
            lines.extend(l.last(3, |t| t.starts_with("live: clankermatch: ")));
            if game.is_none() {
                // Why it never logged on: the drive's last words.
                lines.extend(l.last(3, |t| t.starts_with("bots-drive")));
            }
            let detail = match (&server, &game) {
                (Some((_, st)), _) if !has(*st, SERVER_MARKS) => format!(
                    "the server's marks lack hopper {} ({}): its hopper file is not the built-in \
                     lineup (a published one replaced it?)",
                    plan.hopper, plan.title
                ),
                (None, _) => "no `bots-test: server clanker hoppers` line: the harness could not \
                              read the server's marks"
                    .into(),
                (_, Some((_, gt))) => format!(
                    "the game's marks ({}) lack hopper {}",
                    &gt[GAME_MARKS.len()..],
                    plan.hopper
                ),
                (_, None) => match super::picker_openings(&l.texts) {
                    0 => "the game never said `live: clankermatch: clanker hoppers [..]`: the \
                          HLE asks at logon (`GET /v1/hoppers/clankers`); its line above says \
                          why not, or it never signed in"
                        .into(),
                    n => format!(
                        "the game never said `live: clankermatch: clanker hoppers [..]`: it \
                         never logged on; the account picker (dialog 0x14, \
                         `dialog_live_signin`) opened {n} time(s) and the profile never \
                         signed in (A on its row 0 is \"no account\"; the run's account is \
                         row 1)"
                    ),
                },
            };
            check(
                "clanker hoppers listed (server and game)",
                false,
                detail,
                lines,
            )
        }
    });

    // 3. The Clankermatch row: the playlist list is the clanker hoppers.
    let empty = l.first(|t| t.starts_with(EMPTY_LIST));
    checks.push(match (l.first(|t| t.starts_with(ACTIVE)), empty) {
        (Some((line, t)), None) => check(
            "the Clankermatch row opened the clanker playlists",
            true,
            t.into(),
            vec![line],
        ),
        (_, Some((line, _))) => check(
            "the Clankermatch row opened the clanker playlists",
            false,
            "the playlist list was empty: the game had no clanker hoppers when it built it (the \
             marks came after the list, or never)"
                .into(),
            vec![line],
        ),
        (None, None) => {
            let mut lines = l.last(2, |t| t.starts_with(HUMAN_SEARCH));
            lines.extend(drive_lines(l));
            check(
                "the Clankermatch row opened the clanker playlists",
                false,
                "no `clankermatch: the playlists are the clanker hoppers`: the Live menu's row 2 \
                 (Clankermatch) was never pressed; the last presses and screen follow"
                    .into(),
                lines,
            )
        }
    });

    // 4. The search started, of the planned hopper.
    let search = l.index_from(0, |t| t.starts_with(SEARCH));
    let searched_hopper = |i: usize| {
        l.texts[i][SEARCH.len()..]
            .split(':')
            .next()
            .and_then(|h| h.parse::<u32>().ok())
    };
    checks.push(match search {
        Some(i) if searched_hopper(i) == Some(plan.hopper) => check(
            "the clanker search started",
            true,
            l.texts[i]["live: clankermatch: ".len()..].to_string(),
            vec![l.raw_at(i)],
        ),
        Some(i) => check(
            "the clanker search started",
            false,
            format!(
                "it searched clanker playlist {:?}, not {} ({}): the drive picked row {} but the \
                 list's rows are not in hopper order",
                searched_hopper(i),
                plan.hopper,
                plan.title,
                plan.row
            ),
            vec![l.raw_at(i)],
        ),
        None => {
            let human = l.first(|t| t.starts_with(HUMAN_SEARCH));
            let detail = if human.is_some() {
                "a human playlist was searched (`live: optimatch: searching playlist`), not a \
                 clanker one: the list was OptiMatch's (Clankermatch off), or the HLE did not \
                 take the hopper for a clanker hopper (its marks)"
                    .to_string()
            } else {
                "no `live: clankermatch: searching clanker playlist`: no search started (A on \
                 the playlist never went, or opened something else)"
                    .to_string()
            };
            let mut lines: Vec<String> = human.into_iter().map(|h| h.0).collect();
            lines.extend(drive_lines(l));
            check("the clanker search started", false, detail, lines)
        }
    });

    // 5. The search ended with this console hosting the clanker
    // playlist's lobby: an advertisement after the search, and the Clankers
    // driver's fill for that session.
    let host_at = search.and_then(|s| l.index_from(s, |t| hosting(t).is_some()));
    let advert =
        |session: &str| search.and_then(|s| l.index_from(s, |t| advertised_id(t) == Some(session)));
    checks.push(match host_at {
        Some(h) => {
            let (hopper, session) = hosting(l.texts[h]).expect("found by it");
            let ad = advert(session);
            let mut lines: Vec<String> = ad.map(|a| l.raw_at(a)).into_iter().collect();
            lines.push(l.raw_at(h));
            let pass = hopper == plan.hopper && ad.is_some();
            check(
                "the search ended with this console hosting",
                pass,
                if pass {
                    format!("session {session} advertised; the Clankers fill hosts hopper {hopper}")
                } else if hopper != plan.hopper {
                    format!("hosting hopper {hopper}, not {}", plan.hopper)
                } else {
                    format!("the fill names session {session}, but no `live: session {session} advertised` came after the search")
                },
                lines,
            )
        }
        None => {
            let after = search.unwrap_or(0);
            let ad = l.index_from(after, |t| advertised_id(t).is_some());
            let detail = match (search, ad) {
                (None, _) => "no search, so no hosting".to_string(),
                (Some(_), Some(_)) => "a session was advertised after the search, but the \
                                       Clankers driver never said `clankermatch: hosting clanker \
                                       playlist`: the HLE did not read the session's hopper as a \
                                       clanker one (`clanker_hosted`: the advertisement's \
                                       playlist attribute, or the marks)"
                    .to_string(),
                (Some(_), None) => "the search started but no session was advertised after it: \
                                    the game kept searching (the held search never answered?) \
                                    or never hosted"
                    .to_string(),
            };
            let mut lines: Vec<String> = ad.map(|a| l.raw_at(a)).into_iter().collect();
            lines.extend(drive_lines(l));
            check(
                "the search ended with this console hosting",
                false,
                detail,
                lines,
            )
        }
    });

    // 6. The fill's plan: phantoms in the match, never the party.
    let split = l.first(|t| phantoms_planned(t).is_some());
    checks.push(match split {
        Some((line, t)) => {
            let n = phantoms_planned(t).expect("found by it");
            check(
                "phantoms planned for the lobby",
                n == plan.bots,
                if n == plan.bots {
                    t[FILL_PREFIX.len()..].to_string()
                } else {
                    format!(
                        "{n} phantoms, not the {} the plan works out ({} players, {} teams)",
                        plan.bots, plan.max, plan.teams
                    )
                },
                vec![line],
            )
        }
        None => check(
            "phantoms planned for the lobby",
            false,
            "no `clankers: clankermatch: N phantoms join the lobby`: the fill never started \
             (step 5)"
                .into(),
            l.last(3, |t| t.starts_with("clankers: ")),
        ),
    });

    // 7. No bot in the party: every bot is a phantom, so none signs in
    // on a controller port (docs/game/bots.md §8.4).
    {
        let signed: Vec<String> = l
            .texts
            .iter()
            .enumerate()
            .filter(|(_, t)| super::signing_port(t).is_some())
            .map(|(i, _)| l.raw_at(i))
            .collect();
        checks.push(check(
            "no bot in the party: none signs in on a controller port",
            signed.is_empty(),
            if signed.is_empty() {
                "no `bots: signing ...` line: every bot is a phantom in the match".into()
            } else {
                "a bot signed in on a controller port: it shows in the party, and the \
                 matchmade start checks its controller"
                    .into()
            },
            signed,
        ));
    }

    // 8. The phantoms are the roster's bot accounts.
    {
        let rostered: Vec<(String, &str)> = l
            .texts
            .iter()
            .enumerate()
            .filter(|(_, t)| t.starts_with("clankers: ") && t.contains(" (phantom, rostered) on"))
            .map(|(i, t)| (l.raw_at(i), *t))
            .collect();
        let want = plan.phantoms();
        let n = rostered.len();
        checks.push(check(
            "the phantoms are the roster's bot accounts",
            n >= want && want > 0,
            if n >= want {
                format!("{n} rostered phantoms")
            } else {
                format!(
                    "{n} of {want} phantoms rostered (`clankers: G (phantom, rostered) on`): \
                     Live's roster refused or never came (`no roster from Live`), so host-made \
                     bots with no account"
                )
            },
            rostered.into_iter().map(|r| r.0).collect(),
        ));
    }

    // 9. The lobby's player count reached the playlist's fewest players,
    // before the game started: the game's own count of its session
    // (`bots: lobby players: N in the game's session`), not the Live
    // server's, which counts what the host told it (cm-run2's "4 joined"
    // came with no bot in the game's lobby).
    let start = host_at.and_then(|h| l.index_from(h, game_start));
    let lobby_end = start.unwrap_or(l.texts.len());
    let counts: Vec<(usize, u64)> = (from..lobby_end)
        .filter_map(|i| read_lobby_players(l.texts[i]).map(|n| (i, n)))
        .collect();
    let best = counts.iter().max_by_key(|c| c.1);
    let min = u64::from(plan.min);
    checks.push(match best {
        Some(&(i, n)) => check(
            "the lobby reached the playlist's fewest players",
            n >= min,
            if n >= min {
                format!("{n} players in the game's lobby (the playlist starts at {min})")
            } else {
                format!(
                    "at most {n} players in the game's lobby of the {min} the playlist starts \
                     at: no phantom joined the lobby (step 12 says why)"
                )
            },
            vec![l.raw_at(i)],
        ),
        None => {
            let mut lines = l.last(2, |t| t.starts_with("bots: lobby facts"));
            lines.extend(l.last(2, |t| t.starts_with("bots-test: session ")));
            check(
                "the lobby reached the playlist's fewest players",
                false,
                "no `bots: lobby players: N` while the lobby was up: the game never read a \
                 lobby the bots can join (the lobby facts line says why; the Live server's \
                 session count is not the game's)"
                    .into(),
                lines,
            )
        }
    });

    // 10. The countdown ran and the game started: a multiplayer map
    // loading or playing after the lobby was hosted (cm-run2's "phase
    // loading" was the main menu coming back, `map mainmenu`).
    checks.push(match start {
        Some(i) => check(
            "the countdown ran and the game started",
            true,
            read_status(l.texts[i])
                .map(|s| format!("phase {} on {} after the lobby", s.0, s.2))
                .unwrap_or_default(),
            vec![l.raw_at(i)],
        ),
        None => {
            let reached = best.is_some_and(|b| b.1 >= min);
            let mut lines = l.last(2, |t| t.starts_with("bots-test: status"));
            lines.extend(l.last(2, |t| t.starts_with("bots-drive")));
            check(
                "the countdown ran and the game started",
                false,
                if host_at.is_none() {
                    "no lobby, so no start".into()
                } else if reached {
                    "the lobby had its fewest players but no multiplayer map loaded: the \
                     game's matchmaking start (its countdown, or the party leader check) never \
                     ran"
                    .into()
                } else {
                    "no multiplayer map loaded: the lobby stayed short of its fewest players \
                     (step 9)"
                        .into()
                },
                lines,
            )
        }
    });

    // 11. Gameplay on a map.
    let play = start.and_then(|s| {
        l.index_from(s, |t| {
            read_status(t).is_some_and(|(phase, mode, _)| phase == "gameplay" && mode != "campaign")
        })
    });
    checks.push(match play {
        Some(i) => check(
            "gameplay on a map",
            true,
            read_status(l.texts[i])
                .map(|s| format!("on {} ({})", s.2, s.1))
                .unwrap_or_default(),
            vec![l.raw_at(i)],
        ),
        None => check(
            "gameplay on a map",
            false,
            "the agent API never answered phase gameplay in a multiplayer game after the lobby"
                .into(),
            l.last(3, |t| t.starts_with("bots-test: status")),
        ),
    });

    // 12. The phantoms joined the lobby, before the start (§5.1,
    // "Phantoms in the pregame lobby").
    let want = plan.phantoms();
    let joins: Vec<usize> = (0..l.texts.len())
        .filter(|&i| phantom_joined(l.texts[i]).is_some())
        .collect();
    // Who joined the lobby before the start, and who only after it.
    let mut early = std::collections::BTreeSet::new();
    let mut late = std::collections::BTreeSet::new();
    for &i in &joins {
        let Some(name) = phantom_joined(l.texts[i]) else {
            continue;
        };
        if phantom_joined_lobby(l.texts[i]).is_some() && start.is_none_or(|s| i < s) {
            early.insert(name);
        } else if start.is_some_and(|s| i > s) {
            late.insert(name);
        }
    }
    let late: Vec<&str> = late.difference(&early).copied().collect();
    let early: Vec<&str> = early.into_iter().collect();
    let failed = l.last(3, phantom_failed);
    let pass = early.len() >= want && failed.is_empty();
    let mut lines: Vec<String> = joins
        .iter()
        .rev()
        .take(want.max(1))
        .rev()
        .map(|&i| l.raw_at(i))
        .collect();
    lines.extend(failed.iter().cloned());
    if !pass {
        lines.extend(l.last(3, |t| {
            t.starts_with("bots: phantom ") && t.contains(" waits")
        }));
    }
    checks.push(check(
        "phantoms joined the lobby before the start",
        pass,
        if pass && want == 0 {
            "none planned".into()
        } else if pass {
            format!("{} joined in the lobby: {}", early.len(), early.join(", "))
        } else if !failed.is_empty() {
            "a phantom failed".into()
        } else if !late.is_empty() {
            format!(
                "{} of {want} joined the lobby; {} joined only once the game ran: the lobby \
                 join is off (H2_PHANTOM_LOBBY=0) or never saw this console host the lobby",
                early.len(),
                late.len()
            )
        } else {
            format!(
                "{} of {want} joined the lobby (`bots: phantom NAME joins (peer N, in the \
                 pregame lobby)`); they join the lobby this console hosts, so a wait line says \
                 why not",
                early.len()
            )
        },
        lines,
    ));

    // 13. The game record: kind clankermatch, its playlist, the bots.
    let record = l
        .texts
        .iter()
        .enumerate()
        .filter_map(|(i, t)| read_game(t).map(|g| (i, g)))
        .max_by_key(|(_, (b, _, _))| *b);
    checks.push(match record {
        Some((i, (b, kind, playlist))) => {
            let pass = kind == "clankermatch" && b >= plan.bots.max(1);
            check(
                "game record: kind clankermatch, the bots with bot: true",
                pass,
                if pass {
                    format!("{playlist}: {b} bots of the {} planned", plan.bots)
                } else if kind != "clankermatch" {
                    format!(
                        "recorded as {kind:?} (playlist {playlist}), not clankermatch: the game's \
                         telemetry did not see a clanker hopper's playlist"
                    )
                } else {
                    format!("only {b} players with bot: true")
                },
                vec![l.raw_at(i)],
            )
        }
        None => check(
            "game record: kind clankermatch, the bots with bot: true",
            false,
            "the Live server has no game record from this console".into(),
            Vec::new(),
        ),
    });

    // 14. After the game, the bots go: the fill ends once this console
    // hosts no clanker session (30 s after the last), and each phantom
    // comes off.
    let after = play.unwrap_or(l.texts.len());
    let gone = l.index_from(after, |t| BOTS_GO.contains(&t));
    checks.push(match gone {
        Some(g) => {
            let mut names = std::collections::BTreeSet::new();
            let offs: Vec<usize> = (g..l.texts.len())
                .filter(|&i| phantom_off(l.texts[i]).is_some_and(|name| names.insert(name)))
                .collect();
            let pass = offs.len() >= plan.bots;
            let mut lines = vec![l.raw_at(g)];
            lines.extend(offs.iter().map(|&i| l.raw_at(i)));
            check(
                "after the game, the phantoms leave",
                pass,
                if pass {
                    format!("the fill ended; {} phantoms off", offs.len())
                } else {
                    format!(
                        "the fill ended but only {} of {} phantoms said `NAME off`",
                        offs.len(),
                        plan.bots
                    )
                },
                lines,
            )
        }
        None => {
            let mut lines = l.last(2, |t| t.starts_with("live: session "));
            lines.extend(drive_lines(l));
            check(
                "after the game, the phantoms leave",
                false,
                if play.is_none() {
                    "no game, so nothing to leave".into()
                } else {
                    "no `clankermatch: the game is over: the bots go` after the game: the console \
                     still hosts the clanker session (the drive did not leave the lobby, or a \
                     new game started), or it left less than 30 s before the run ended"
                        .into()
                },
                lines,
            )
        }
    });

    let trouble = l.last(8, |t| super::TROUBLE.iter().any(|k| t.contains(*k)));
    Verdict {
        title: format!(
            "bots test: Clankermatch, {} (hopper {}): {} bots ({} phantoms) with {}",
            plan.title,
            plan.hopper,
            plan.bots,
            plan.phantoms(),
            plan.tag
        ),
        plan: Plan {
            count: plan.bots,
            tag: plan.tag.clone(),
            map: None,
        },
        checks,
        trouble,
    }
}

#[cfg(test)]
mod tests;
