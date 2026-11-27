//! The co-op test's log and checklist (`h2 coop test`, `h2 coop verdict`;
//! docs/live/live-coop.md section 12): which lines of each game go into the
//! merged log, how a merged line reads, and which checklist steps a log
//! shows.
//!
//! A merged log starts with one `# host <gamertag>` line, a
//! `# guest <gamertag>` line per guest and a `# late <gamertag>` line per
//! guest that joins once the host is in the level (`--late`), then one line
//! per game line: `[+SECONDS] GAMERTAG | TEXT`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// Whether a game's stderr line goes into the merged log: the co-op
/// lines, not the boot-time dumps that merely name a co-op hook (the
/// feature plan, a hook's first `active`, the health model's `game start`).
pub fn wanted(line: &str) -> bool {
    const DROP: [&str; 3] = ["feature plan:", "feature: ", "health: "];
    const DROP_ANY: [&str; 1] = ["hooks installed via feature plan"];
    const KEEP: [&str; 20] = [
        "coop-live",
        "coop-jip",
        "coop-probe",
        "coop-auto",
        // The players 3 to 16 placed (or REFUSED), the checkpoint each
        // game saves, and the engine's own desync latch.
        "coop-players",
        "checkpoint save",
        "OUT OF SYNC",
        "skulls",
        "session",
        "game start: options",
        "live: playing as",
        "custom games",
        "agent: listening",
        "panicked",
        // The join's steps (live-hle): lookup, QoS, keys.
        "live: join",
        "live: qos",
        "live: xnet",
        "guest thread exited",
        // `--drive`: the screens (H2_UI_WATCH) and the presses.
        "ui: screen",
        "coop-drive",
    ];
    !DROP.iter().any(|d| line.starts_with(d))
        && !DROP_ANY.iter().any(|d| line.contains(d))
        && KEEP.iter().any(|k| line.contains(k))
}

/// One merged line.
pub fn format_line(seconds: f64, tag: &str, text: &str) -> String {
    format!("[+{seconds:.3}] {tag} | {text}")
}

/// A merged line's gamertag and text.
pub fn parse_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("[+")?;
    let (_, rest) = rest.split_once("] ")?;
    rest.split_once(" | ")
}

/// The roles a merged log's header names.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Roles {
    pub host: String,
    pub guests: Vec<String>,
    /// Guests that join in progress: not in the lobby steps, checked by
    /// [`Verdict::joins`] instead.
    pub late: Vec<String>,
}

impl Roles {
    pub fn header(&self) -> String {
        let mut s = format!("# host {}\n", self.host);
        for g in &self.guests {
            let _ = writeln!(s, "# guest {g}");
        }
        for g in &self.late {
            let _ = writeln!(s, "# late {g}");
        }
        s
    }

    pub fn read(log: &str) -> Option<Roles> {
        let mut r = Roles::default();
        for line in log.lines() {
            if let Some(t) = line.strip_prefix("# host ") {
                r.host = t.trim().into();
            } else if let Some(t) = line.strip_prefix("# guest ") {
                r.guests.push(t.trim().into());
            } else if let Some(t) = line.strip_prefix("# late ") {
                r.late.push(t.trim().into());
            }
        }
        (!r.host.is_empty()).then_some(r)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Host,
    Guests,
    /// The host and the guests who joined in the lobby.
    Lobby,
    Everyone,
}

/// What a step counts, beyond a game printing a line holding all of `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// Each game it is for printed such a line.
    Line,
    /// The host invited every guest: one `coop-auto: invited <tag>` per guest.
    Invited,
    /// The line's `for K consoles` is the run's N.
    Consoles,
    /// The line's `campaign with K peers` is the run's N.
    Peers,
    /// Each game's first `checkpoint save at tick T` has the same T.
    Checkpoint,
    /// The host placed players 3 to N (`coop-players: player I placed`) and
    /// no game had one REFUSED.
    Placed,
    /// No game printed `all`; a game that did is missing.
    Never,
}

/// A checklist step: seen when every game it is for printed a line holding
/// all of `all` (and `rule` holds). `auto` steps only print with `--auto`.
struct Step {
    what: &'static str,
    who: Who,
    all: &'static [&'static str],
    auto: bool,
    rule: Rule,
}

/// docs/live/live-coop.md sections 11.3 and 9.2, in the order a run passes
/// them.
const STEPS: &[Step] = &[
    Step {
        what: "signed in to Live",
        who: Who::Everyone,
        all: &["live: playing as "],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "the disc's campaign levels read",
        who: Who::Everyone,
        all: &["coop-live: ", " campaign levels ("],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "Co-op Campaign opens the game's own lobby",
        who: Who::Host,
        all: &["coop-live: Live menu row ", "in co-op mode"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "hosting a co-op campaign in that lobby",
        who: Who::Host,
        all: &["coop-live: hosting a co-op campaign in the game's own lobby"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "the lobby is in co-op mode",
        who: Who::Host,
        all: &[
            "coop-live: the Co-op Campaign row's lobby: mode ",
            "(co-op campaign)",
        ],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "level and difficulty advertised",
        who: Who::Host,
        all: &["coop-live: the lobby holds ", "; advertised"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "the host invited every guest",
        who: Who::Host,
        all: &["coop-auto: invited "],
        auto: true,
        rule: Rule::Invited,
    },
    Step {
        what: "the guest accepted",
        who: Who::Guests,
        all: &["coop-auto: accepting "],
        auto: true,
        rule: Rule::Line,
    },
    Step {
        what: "the guest is in the host's co-op session",
        who: Who::Guests,
        all: &["coop-live: guest: joined "],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "ping between the two",
        who: Who::Everyone,
        all: &["coop-live: ping ", " ms"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "Start Game: the co-op check passed for every console",
        who: Who::Host,
        all: &["coop-live: lobby start state 7 ", " answered "],
        auto: false,
        rule: Rule::Consoles,
    },
    Step {
        what: "the session start counted the peers",
        who: Who::Host,
        all: &["session start: campaign with "],
        auto: false,
        rule: Rule::Peers,
    },
    Step {
        what: "the session started",
        who: Who::Lobby,
        all: &["session start returned 1"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "the host simulates (sync-server)",
        who: Who::Host,
        all: &["simulation mode sync-server"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "the guest follows (sync-client)",
        who: Who::Guests,
        all: &["simulation mode sync-client"],
        auto: false,
        rule: Rule::Line,
    },
    Step {
        what: "every game saved its checkpoint at the same tick",
        who: Who::Lobby,
        all: &["checkpoint save at tick "],
        auto: false,
        rule: Rule::Checkpoint,
    },
    Step {
        what: "players 3 to N placed, none refused",
        who: Who::Host,
        all: &["coop-players: player "],
        auto: false,
        rule: Rule::Placed,
    },
    Step {
        what: "no game went out of sync",
        who: Who::Everyone,
        all: &["OUT OF SYNC"],
        auto: false,
        rule: Rule::Never,
    },
];

/// Lines that mean something went wrong, whatever the step.
const TROUBLE: [&str; 10] = [
    "OUT OF SYNC",
    "coop-jip: back to the lobby",
    "coop-live: ping: no answer",
    "returned 0 (refused)",
    "coop-live: refused",
    "could not invite",
    "could not accept",
    "did not open",
    "panicked",
    "coop-live: off:",
];

/// Whether a step is seen, not seen, or one nothing prints without
/// `--auto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    Yes,
    No { missing: Vec<String> },
    NotLogged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub n: usize,
    pub what: &'static str,
    pub seen: Seen,
    /// For a step not seen: the likely reason, in plain words (most often
    /// a press nobody made yet).
    pub why: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub steps: Vec<StepResult>,
    /// One line per late guest: whether and when it joined in progress
    /// (docs/live/live-coop.md section 14), then the host's admissions.
    pub joins: Vec<Join>,
    /// A run whose host started alone (`--solo-start`: no lobby guests,
    /// every guest late): the solo start's steps
    /// (docs/live/live-coop-join.md 13.4). Empty otherwise.
    pub solo: Vec<Join>,
    /// Merged lines that look like trouble, as they are.
    pub trouble: Vec<String>,
}

/// The checklist `log` (a merged log) shows. `auto` says whether the run
/// had `--auto` (its steps are otherwise [`Seen::NotLogged`] unless seen).
pub fn verdict(log: &str, auto: bool) -> Option<Verdict> {
    let roles = Roles::read(log)?;
    let lines: Vec<(&str, &str)> = log.lines().filter_map(parse_line).collect();
    let mut steps = Vec::new();
    for (i, step) in STEPS.iter().enumerate() {
        let games: Vec<&str> = match step.who {
            Who::Host => vec![roles.host.as_str()],
            Who::Guests => roles.guests.iter().map(String::as_str).collect(),
            Who::Lobby => std::iter::once(roles.host.as_str())
                .chain(roles.guests.iter().map(String::as_str))
                .collect(),
            Who::Everyone => std::iter::once(roles.host.as_str())
                .chain(roles.guests.iter().map(String::as_str))
                .chain(roles.late.iter().map(String::as_str))
                .collect(),
        };
        // N: the games in the lobby, host included.
        let n = 1 + roles.guests.len();
        let has = |g: &str| {
            lines.iter().any(|(tag, text)| {
                *tag == g
                    && step.all.iter().all(|p| text.contains(p))
                    && !text.contains("ping: no answer")
            })
        };
        let missing: Vec<String> = match step.rule {
            Rule::Line => games
                .into_iter()
                .filter(|g| !has(g))
                .map(String::from)
                .collect(),
            Rule::Never => games
                .into_iter()
                .filter(|g| has(g))
                .map(|g| format!("{g} (OUT OF SYNC)"))
                .collect(),
            Rule::Invited => {
                let invited: BTreeSet<&str> = lines
                    .iter()
                    .filter(|(tag, _)| *tag == roles.host)
                    .filter_map(|(_, t)| t.split_once("coop-auto: invited "))
                    .filter_map(|(_, rest)| rest.split_whitespace().next())
                    .collect();
                let missing = roles
                    .guests
                    .iter()
                    .filter(|g| !invited.contains(g.as_str()))
                    .cloned()
                    .collect();
                tally(missing, invited.len(), n - 1, "invitations")
            }
            // " console" and " peer" read "1 console" and "3 consoles" alike.
            Rule::Consoles => counted(&lines, &roles.host, step.all, " console", " for ", n),
            Rule::Peers => counted(&lines, &roles.host, step.all, " peer", "campaign with ", n),
            Rule::Checkpoint => {
                let ticks: Vec<(&str, Option<&str>)> = games
                    .iter()
                    .map(|g| {
                        let t = lines
                            .iter()
                            .filter(|(tag, _)| tag == g)
                            .find_map(|(_, t)| t.split_once("checkpoint save at tick "))
                            .and_then(|(_, rest)| rest.split_whitespace().next());
                        (*g, t)
                    })
                    .collect();
                let first = ticks.iter().find_map(|(_, t)| *t);
                ticks
                    .into_iter()
                    .filter(|(_, t)| t.is_none() || *t != first)
                    .map(|(g, t)| match t {
                        Some(t) => format!("{g} (tick {t}, not {})", first.unwrap_or("?")),
                        None => g.to_string(),
                    })
                    .collect()
            }
            Rule::Placed => {
                let mut out = Vec::new();
                for (tag, text) in &lines {
                    if text.contains("coop-players: ") && text.contains("REFUSED") {
                        out.push(format!("{tag} (REFUSED)"));
                    }
                }
                let placed: BTreeSet<&str> = lines
                    .iter()
                    .filter(|(tag, _)| *tag == roles.host)
                    .filter_map(|(_, t)| t.split_once("coop-players: player "))
                    .filter(|(_, rest)| rest.contains(" placed") && !rest.contains("REFUSED"))
                    .filter_map(|(_, rest)| rest.split_whitespace().next())
                    .collect();
                // Players of index 2 and up are the ones placed.
                let want = n.saturating_sub(2);
                if out.is_empty() && placed.len() < want {
                    out.push(format!(
                        "{} ({} of {want} placed)",
                        roles.host,
                        placed.len()
                    ));
                }
                out
            }
        };
        let seen = if missing.is_empty() {
            Seen::Yes
        } else if step.auto && !auto {
            Seen::NotLogged
        } else {
            Seen::No { missing }
        };
        let why = match &seen {
            Seen::No { missing } => why(i + 1, missing, &roles, &lines, auto),
            _ => None,
        };
        steps.push(StepResult {
            n: i + 1,
            what: step.what,
            seen,
            why,
        });
    }
    let trouble = log
        .lines()
        .filter(|l| parse_line(l).is_some_and(|(_, t)| TROUBLE.iter().any(|p| t.contains(p))))
        .map(String::from)
        .collect();
    let joins = joins(&lines, &roles);
    let solo = solo(&lines, &roles);
    Some(Verdict {
        steps,
        joins,
        solo,
        trouble,
    })
}

const SOLO_ANSWER: &str = " for 1 console (alone";
const SOLO_START: &str = "session start: campaign with 1 peer (alone)";
const JIP_LIFT: &str = "coop-jip: host: a console joins the running game";
const JOIN_PLACED: &str = " joins in progress: placed next to player ";
const JOIN_REFUSED: &str = " joins in progress: REFUSED";
const REVERT: &str = "coop-probe: checkpoint revert";

/// The solo start's steps, for a run with no lobby guests and late ones:
/// the host started alone, as a co-op game, let each late console into the
/// running game, placed each newcomer by the squad, and no game reverted
/// after the first join.
fn solo(lines: &[(&str, &str)], roles: &Roles) -> Vec<Join> {
    if !roles.guests.is_empty() || roles.late.is_empty() {
        return Vec::new();
    }
    let host = roles.host.as_str();
    let want = roles.late.len();
    let count = |p: &str| {
        lines
            .iter()
            .filter(|(t, text)| *t == host && text.contains(p))
            .count()
    };
    let check = |ok: bool, says: String| Join {
        tag: host.to_string(),
        joined: ok,
        says,
    };
    let mut out = vec![
        check(
            said(lines, host, SOLO_ANSWER),
            if said(lines, host, SOLO_ANSWER) {
                format!("{host} started alone: the lobby's co-op check answered for 1 console")
            } else if said(lines, host, "coop-live: lobby start state 7") {
                format!("{host}'s lobby kept the co-op check (is coop.join_in_progress off?)")
            } else {
                format!("nobody pressed Start Game in {host}'s lobby")
            },
        ),
        check(
            said(lines, host, SOLO_START),
            if said(lines, host, SOLO_START) {
                format!("{host}'s game is co-op with one player (two shown until OPTIONS_NEW)")
            } else {
                format!("{host}'s session start never showed two players: the game may be solo")
            },
        ),
    ];
    let let_in = count(JIP_LIFT);
    out.push(check(
        let_in >= want,
        format!("{host} stayed joinable: let {let_in} of {want} console(s) into the running game"),
    ));
    let placed = count(JOIN_PLACED);
    let refused = count(JOIN_REFUSED);
    out.push(check(
        placed >= want && refused == 0,
        if refused == 0 {
            format!("{placed} of {want} newcomer(s) placed next to the squad")
        } else {
            format!("{placed} of {want} newcomer(s) placed next to the squad; {refused} REFUSED")
        },
    ));
    let following: Vec<&str> = roles
        .late
        .iter()
        .map(String::as_str)
        .filter(|g| !said(lines, g, "simulation mode sync-client"))
        .collect();
    out.push(check(
        following.is_empty(),
        if following.is_empty() {
            "every newcomer follows the host (sync-client)".into()
        } else {
            format!("never a sync-client: {}", following.join(", "))
        },
    ));
    let first_join = lines.iter().position(|(t, text)| *t == host && text.contains(JIP_LIFT));
    let reverted: Vec<&str> = match first_join {
        Some(i) => {
            let mut v: Vec<&str> = lines[i..]
                .iter()
                .filter(|(_, text)| text.contains(REVERT))
                .map(|(t, _)| *t)
                .collect();
            v.dedup();
            v
        }
        None => Vec::new(),
    };
    out.push(check(
        first_join.is_some() && reverted.is_empty(),
        match (first_join, reverted.is_empty()) {
            (None, _) => "no join to watch for reverts".into(),
            (Some(_), true) => "no game reverted after the first join".into(),
            (Some(_), false) => format!("reverted after a join: {}", reverted.join(", ")),
        },
    ));
    out
}

/// `missing` guests, or a count note when the count is short but no guest
/// can be named.
fn tally(missing: Vec<String>, got: usize, want: usize, what: &str) -> Vec<String> {
    if missing.is_empty() && got < want {
        vec![format!("{got} of {want} {what}")]
    } else {
        missing
    }
}

/// The host's lines holding all of `all`: each has a number between `before`
/// and `after`, and some line's number is `n`. Otherwise the host is missing,
/// with the numbers it logged.
fn counted(
    lines: &[(&str, &str)],
    host: &str,
    all: &[&str],
    after: &str,
    before: &str,
    n: usize,
) -> Vec<String> {
    let counts: Vec<usize> = lines
        .iter()
        .filter(|(tag, text)| *tag == host && all.iter().all(|p| text.contains(p)))
        .filter_map(|(_, text)| {
            let head = text.rsplit_once(before)?.1;
            head.split_once(after.trim_end())?.0.trim().parse().ok()
        })
        .collect();
    if counts.contains(&n) {
        return Vec::new();
    }
    let said = if counts.is_empty() {
        host.to_string()
    } else {
        let c: Vec<String> = counts.iter().map(usize::to_string).collect();
        format!("{host} (said {}, wanted {n})", c.join(", "))
    };
    vec![said]
}

/// A late guest's join in progress, as its lines and the host's show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub tag: String,
    pub joined: bool,
    pub says: String,
}

const JOINED: &str = "coop-jip: joined in progress at tick ";
const BACK: &str = "coop-jip: back to the lobby: ";
const ADMITTED: &str = "coop-jip: admitted player ";

fn joins(lines: &[(&str, &str)], roles: &Roles) -> Vec<Join> {
    let after = |tag: &str, p: &str| {
        lines
            .iter()
            .filter(|(t, _)| *t == tag)
            .find_map(|(_, text)| text.split_once(p).map(|(_, rest)| rest.to_string()))
    };
    let mut out = Vec::new();
    for tag in &roles.late {
        let (joined, says) = if let Some(rest) = after(tag, JOINED) {
            // "4322 as player 3"
            let (tick, player) = rest.split_once(" as ").unwrap_or((rest.as_str(), ""));
            let player = if player.is_empty() {
                String::new()
            } else {
                format!(" ({player})")
            };
            (
                true,
                format!("{tag} joined in progress at tick {tick}{player}"),
            )
        } else if let Some(why) = after(tag, BACK) {
            (false, format!("{tag} did not join in progress: {why}"))
        } else if said(lines, tag, "coop-auto: accepting ") {
            (
                false,
                format!(
                    "{tag} accepted but never said it joined in progress (was {} in a level?)",
                    roles.host
                ),
            )
        } else {
            (false, format!("{tag} never accepted an invitation"))
        };
        out.push(Join {
            tag: tag.clone(),
            joined,
            says,
        });
    }
    for (t, text) in lines {
        if *t == roles.host
            && let Some((_, rest)) = text.split_once(ADMITTED)
        {
            out.push(Join {
                tag: roles.host.clone(),
                joined: true,
                says: format!("{} admitted player {rest}", roles.host),
            });
        }
    }
    out
}

/// Whether game `tag` printed a line holding `p`.
fn said(lines: &[(&str, &str)], tag: &str, p: &str) -> bool {
    lines.iter().any(|(t, text)| *t == tag && text.contains(p))
}

/// The Xbox Live menu was on `tag`'s screen: its tag was extended, or one
/// of our rows drawn (`custom games: ...`, crates/extensions/game/src/layout_glue.rs).
fn opened_live_menu(lines: &[(&str, &str)], tag: &str) -> bool {
    said(lines, tag, "custom games: xbox live menu tag")
        || said(lines, tag, "custom games: row text")
        || said(lines, tag, "custom games: row chosen")
        || said(lines, tag, "coop-live: Live menu row ")
}

/// Whether `tag` walked any of the host's menu path: picked a lobby mode,
/// a campaign's difficulty (the Skulls page), one of our Live rows, Co-op
/// Campaign, or a lobby start.
fn host_path(lines: &[(&str, &str)], tag: &str) -> bool {
    const PATH: [&str; 7] = [
        "coop-live: lobby mode",
        "skulls page:",
        "coop-live: Live menu row ",
        "coop-live: hosting",
        "custom games: row chosen",
        "coop-live: the lobby holds",
        "coop-live: lobby start state",
    ];
    PATH.iter().any(|p| said(lines, tag, p))
}

/// Why step `n` was not seen on the games `missing`, when the log says
/// more than "not seen": usually the press before it that nobody made.
fn why(
    n: usize,
    missing: &[String],
    roles: &Roles,
    lines: &[(&str, &str)],
    auto: bool,
) -> Option<String> {
    let host = roles.host.as_str();
    let first = missing.first()?.as_str();
    let not_online: Vec<&str> = roles
        .guests
        .iter()
        .map(String::as_str)
        .filter(|g| !opened_live_menu(lines, g))
        .collect();
    // A guest walking the host's path (a campaign, a lobby, Co-op
    // Campaign) while the host walked none of it: the wrong window.
    if (3..=6).contains(&n) && !host_path(lines, host) {
        let drove: Vec<&str> = roles
            .guests
            .iter()
            .map(String::as_str)
            .filter(|g| host_path(lines, g))
            .collect();
        if !drove.is_empty() {
            return Some(format!(
                "you drove {}'s window (it started a campaign or lobby itself); drive {host} (host), \
                 the leftmost window, titled \"Halo 2 — {host} (host)\"",
                drove.join(" and ")
            ));
        }
    }
    match n {
        1 => Some(format!(
            "{first} never signed in: it was still building or starting, or its window closed first"
        )),
        2 if said(lines, first, "cannot read the disc's campaign levels") => {
            Some(format!("{first} could not read the disc's campaign levels"))
        }
        2 => Some(format!("{first}'s co-op hooks never started")),
        3 if said(lines, host, "co-op hooks are off")
            || said(lines, host, "co-op screens are off") =>
        {
            Some(format!(
                "{host}'s co-op hooks are off, so Co-op Campaign opened nothing"
            ))
        }
        3 if !opened_live_menu(lines, host) => Some(format!(
            "{host} never opened the Xbox Live menu: on {host}'s window, Xbox Live from the main menu"
        )),
        3 if said(lines, host, "custom games: row chosen") => Some(format!(
            "{host} opened the Xbox Live menu and picked another of its rows, never Co-op Campaign"
        )),
        3 => Some(format!(
            "{host} opened the Xbox Live menu but never picked Co-op Campaign"
        )),
        4 | 5 => Some(format!(
            "{host} picked Co-op Campaign but the lobby did not open in co-op mode \
             (did the game make its Live session? look in {host}.log)"
        )),
        6 => Some(format!(
            "{host}'s lobby never said which level it holds, so there was nothing to invite to"
        )),
        7 if !not_online.is_empty() => Some(format!(
            "{} never opened the Xbox Live menu in their window, so {host} had nobody online to invite",
            not_online.join(", ")
        )),
        7 => Some(format!(
            "{host} saw its friends online but sent no invitation"
        )),
        8 if !said(lines, host, "coop-auto: invited ") => {
            Some(format!("{host} never sent an invitation (step 7)"))
        }
        8 => Some(format!(
            "{first} never accepted: is it signed in to Xbox Live in its window?"
        )),
        9 if auto && said(lines, first, "coop-auto: accepting ") => Some(format!(
            "{first} accepted, but the game did not join {host}'s lobby; open {first}'s friends list \
             to see whether it joins from there"
        )),
        9 => Some(format!(
            "{first} never joined: accept {host}'s invitation in {first}'s friends list (Y in the Xbox Live menu)"
        )),
        11 | 12 if first.contains("said ") => Some(format!(
            "{first}: the run has every console in the lobby, so the host counted the wrong number"
        )),
        11 if !said(lines, host, "coop-live: lobby start state") => {
            Some(format!("nobody pressed Start Game in {host}'s lobby"))
        }
        11 => Some(format!(
            "{host}'s lobby never asked to start co-op with every player in (all players ready?)"
        )),
        _ => None,
    }
}

impl Verdict {
    /// The first step not seen.
    pub fn stopped_at(&self) -> Option<&StepResult> {
        self.steps
            .iter()
            .find(|s| matches!(s.seen, Seen::No { .. }))
    }

    pub fn render(&self) -> String {
        let mut s =
            String::from("co-op over Live, as the logs show it (docs/live/live-coop.md 11.3):\n");
        for step in &self.steps {
            let (mark, note) = match &step.seen {
                Seen::Yes => ("x", String::new()),
                Seen::No { missing } => (" ", format!(": not seen on {}", missing.join(", "))),
                Seen::NotLogged => ("-", ": no line without --auto".into()),
            };
            let _ = writeln!(s, "  [{mark}] {:2}. {}{note}", step.n, step.what);
            if let Some(why) = &step.why {
                let _ = writeln!(s, "         {why}");
            }
        }
        match self.stopped_at() {
            Some(step) => {
                let _ = writeln!(s, "stopped at step {}: {}", step.n, step.what);
                if let Some(why) = &step.why {
                    let _ = writeln!(s, "  {why}");
                }
            }
            None => s.push_str("every step seen\n"),
        }
        if !self.solo.is_empty() {
            s.push_str("solo start (live-coop-join.md 13.4):\n");
            for j in &self.solo {
                let _ = writeln!(s, "  [{}] {}", if j.joined { "x" } else { " " }, j.says);
            }
        }
        if !self.joins.is_empty() {
            s.push_str("join in progress (section 14):\n");
            for j in &self.joins {
                let _ = writeln!(s, "  [{}] {}", if j.joined { "x" } else { " " }, j.says);
            }
        }
        if !self.trouble.is_empty() {
            s.push_str("trouble:\n");
            for t in self.trouble.iter().take(20) {
                let _ = writeln!(s, "  {t}");
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[(&str, &str)]) -> String {
        let roles = Roles {
            host: "Alpha".into(),
            guests: vec!["Bravo".into()],
            late: vec![],
        };
        let mut s = roles.header();
        for (i, (tag, text)) in lines.iter().enumerate() {
            s.push_str(&format_line(i as f64 * 0.5, tag, text));
            s.push('\n');
        }
        s
    }

    #[test]
    fn merged_lines_round_trip_and_the_filter_keeps_the_co_op_lines() {
        let l = format_line(12.3456, "Bravo", "coop-live: ping 4 ms | odd");
        assert_eq!(l, "[+12.346] Bravo | coop-live: ping 4 ms | odd");
        assert_eq!(
            parse_line(&l),
            Some(("Bravo", "coop-live: ping 4 ms | odd"))
        );
        assert_eq!(parse_line("# host Alpha"), None);
        assert!(wanted("coop-probe: session start returned 1"));
        assert!(wanted("skulls: ours on this campaign: Low Gravity"));
        assert!(wanted(
            "live: playing as Alpha (0009000000000001) on http://127.0.0.1:8075"
        ));
        assert!(!wanted("play: slice 1000, guest time 3.0s"));
        assert!(!wanted("d3d: 12 draws"));
    }

    #[test]
    fn roles_come_from_the_header() {
        let r = Roles {
            host: "Alpha".into(),
            guests: vec!["Bravo".into(), "Charlie".into()],
            late: vec![],
        };
        assert_eq!(Roles::read(&r.header()), Some(r));
        assert_eq!(Roles::read("[+1.000] Alpha | x\n"), None);
    }

    #[test]
    fn a_run_that_stopped_before_the_guest_joined() {
        let l = log(&[
            (
                "Alpha",
                "live: playing as Alpha (0009000000000001) on http://127.0.0.1:8075",
            ),
            (
                "Bravo",
                "live: playing as Bravo (0009000000000002) on http://127.0.0.1:8075",
            ),
            ("Alpha", "coop-live: 15 campaign levels (15 named)"),
            ("Bravo", "coop-live: 15 campaign levels (15 named)"),
            (
                "Alpha",
                "coop-live: Live menu row 4 opens the custom game lobby (row 2) in co-op mode",
            ),
            (
                "Alpha",
                "coop-live: hosting a co-op campaign in the game's own lobby",
            ),
            (
                "Alpha",
                "coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)",
            ),
            (
                "Alpha",
                "coop-live: the lobby holds Outskirts on Heroic; advertised",
            ),
            (
                "Alpha",
                "coop-auto: invited Bravo to session 8000000000000001",
            ),
            (
                "Bravo",
                "coop-auto: could not accept Alpha's invitation (0x80150001)",
            ),
            ("Bravo", "coop-live: ping: no answer"),
        ]);
        let v = verdict(&l, true).unwrap();
        let seen: Vec<bool> = v.steps.iter().map(|s| s.seen == Seen::Yes).collect();
        assert_eq!(
            &seen[..8],
            &[true, true, true, true, true, true, true, false]
        );
        assert_eq!(v.stopped_at().unwrap().n, 8);
        assert_eq!(
            v.steps[9].seen,
            Seen::No {
                missing: vec!["Alpha".into(), "Bravo".into()]
            },
            "`ping: no answer` is not a ping"
        );
        assert_eq!(v.trouble.len(), 2);
        let text = v.render();
        assert!(text.contains("[x]  1. signed in to Live"), "{text}");
        assert!(
            text.contains("[ ]  8. the guest accepted: not seen on Bravo"),
            "{text}"
        );
        assert!(
            text.contains("stopped at step 8: the guest accepted"),
            "{text}"
        );
        assert!(text.contains("could not accept"), "{text}");
    }

    #[test]
    fn without_auto_the_invite_steps_are_not_logged_rather_than_missing() {
        let l = log(&[
            ("Alpha", "live: playing as Alpha (1) on x"),
            ("Bravo", "live: playing as Bravo (2) on x"),
        ]);
        let v = verdict(&l, false).unwrap();
        assert_eq!(v.steps[6].seen, Seen::NotLogged);
        assert_eq!(v.steps[7].seen, Seen::NotLogged);
        assert_eq!(v.stopped_at().unwrap().n, 2);
        assert!(
            v.render()
                .contains("[-]  7. the host invited every guest: no line without --auto")
        );
    }

    #[test]
    fn a_whole_run() {
        let l = log(&[
            ("Alpha", "live: playing as Alpha (1) on x"),
            ("Bravo", "live: playing as Bravo (2) on x"),
            ("Alpha", "coop-live: 15 campaign levels (15 named)"),
            ("Bravo", "coop-live: 15 campaign levels (15 named)"),
            (
                "Alpha",
                "coop-live: Live menu row 4 opens the custom game lobby (row 2) in co-op mode",
            ),
            (
                "Alpha",
                "coop-live: hosting a co-op campaign in the game's own lobby",
            ),
            (
                "Alpha",
                "coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)",
            ),
            (
                "Alpha",
                "coop-live: the lobby holds Outskirts on Heroic; advertised",
            ),
            (
                "Alpha",
                "coop-auto: invited Bravo to session 8000000000000001",
            ),
            (
                "Bravo",
                "coop-auto: accepting Alpha's invitation to session 8000000000000001",
            ),
            (
                "Bravo",
                "coop-live: guest: joined Alpha's co-op session 8000000000000001: Outskirts on Heroic",
            ),
            ("Alpha", "coop-live: ping 3 ms"),
            ("Bravo", "coop-live: ping 3 ms"),
            (
                "Alpha",
                "coop-live: lobby start state 7 (co-op wants one console) answered 9 for 2 consoles",
            ),
            (
                "Alpha",
                "coop-probe: session start: campaign with 2 peers: showing the start 1 peer until OPTIONS_NEW",
            ),
            ("Alpha", "coop-probe: session start returned 1"),
            ("Bravo", "coop-probe: session start returned 1"),
            (
                "Alpha",
                "coop-probe: game start: options mode 1, simulation mode sync-server (2), x",
            ),
            (
                "Bravo",
                "coop-probe: game start: options mode 1, simulation mode sync-client (3), x",
            ),
            ("Alpha", "coop-probe: checkpoint save at tick 90"),
            ("Bravo", "coop-probe: checkpoint save at tick 90"),
        ]);
        let v = verdict(&l, true).unwrap();
        assert!(
            v.steps.iter().all(|s| s.seen == Seen::Yes),
            "{}",
            v.render()
        );
        assert!(v.trouble.is_empty());
        assert!(v.render().contains("every step seen"));
    }

    /// The host's log of the 2026-11-17 three-game run up to its main menu
    /// (paths shortened): only the real co-op lines are kept, none of the
    /// feature plan or the hooks' first `active` that name a co-op hook.
    #[test]
    fn the_filter_keeps_the_co_op_lines_of_a_real_run_and_drops_the_plan() {
        let fixture = include_str!("coop_verdict/host-at-menu.log");
        let kept: Vec<&str> = fixture.lines().filter(|l| wanted(l)).collect();
        assert_eq!(
            kept,
            [
                "live: playing as Alpha (0009d7e882ecd47d) on http://127.0.0.1:8075",
                "agent: listening on /Users/u/Library/Caches/h2recomp/coop-alpha.sock (docs/runtime/agent-api.md)",
                "coop-live: co-op takes up to 3 players (coop.max_players)",
                "coop-live: 15 campaign levels (15 named)",
            ]
        );
        assert!(
            fixture
                .lines()
                .any(|l| l.starts_with("feature plan:") && l.contains("coop-live"))
        );
        assert!(
            fixture
                .lines()
                .any(|l| l.starts_with("feature: ") && l.contains("coop-live"))
        );
    }

    fn signed_in() -> Vec<(&'static str, &'static str)> {
        vec![
            ("Alpha", "live: playing as Alpha (1) on x"),
            ("Bravo", "live: playing as Bravo (2) on x"),
            ("Alpha", "coop-live: 15 campaign levels (15 named)"),
            ("Bravo", "coop-live: 15 campaign levels (15 named)"),
        ]
    }

    fn stopped(lines: &[(&str, &str)], auto: bool) -> (usize, String) {
        let v = verdict(&log(lines), auto).unwrap();
        let s = v.stopped_at().unwrap();
        (s.n, s.why.clone().unwrap_or_default())
    }

    #[test]
    fn step_3_says_which_press_the_host_never_made() {
        // The real run: everyone at the main menu, nobody in Xbox Live.
        let (n, why) = stopped(&signed_in(), true);
        assert_eq!(n, 3);
        assert_eq!(
            why,
            "Alpha never opened the Xbox Live menu: on Alpha's window, Xbox Live from the main menu"
        );
        let mut l = signed_in();
        l.push((
            "Alpha",
            "custom games: xbox live menu tag has 6 panes and room for 5 rows",
        ));
        l.push(("Alpha", "custom games: row text active"));
        assert_eq!(
            stopped(&l, true).1,
            "Alpha opened the Xbox Live menu but never picked Co-op Campaign"
        );
        l.push(("Alpha", "custom games: row chosen active"));
        assert!(stopped(&l, true).1.contains("picked another of its rows"));
        let mut off = signed_in();
        off.push((
            "Alpha",
            "coop-live: the co-op hooks are off; nothing opened",
        ));
        assert!(stopped(&off, true).1.contains("co-op hooks are off"));
        // Picked, but the lobby never switched to co-op.
        let mut l = signed_in();
        l.push((
            "Alpha",
            "coop-live: Live menu row 4 opens the custom game lobby (row 2) in co-op mode",
        ));
        l.push((
            "Alpha",
            "coop-live: hosting a co-op campaign in the game's own lobby",
        ));
        let (n, why) = stopped(&l, true);
        assert_eq!(n, 5);
        assert!(
            why.starts_with("Alpha picked Co-op Campaign but the lobby did not open in co-op mode"),
            "{why}"
        );
        assert!(
            verdict(&log(&l), true)
                .unwrap()
                .render()
                .contains("  Alpha picked Co-op Campaign")
        );
    }

    fn in_lobby() -> Vec<(&'static str, &'static str)> {
        let mut l = signed_in();
        l.extend([
            ("Alpha", "coop-live: Live menu row 4 opens the custom game lobby (row 2) in co-op mode"),
            ("Alpha", "coop-live: hosting a co-op campaign in the game's own lobby"),
            ("Alpha", "coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)"),
            ("Alpha", "coop-live: the lobby holds Outskirts on Heroic; advertised"),
        ]);
        l
    }

    #[test]
    fn later_steps_name_the_guests_missing_press() {
        // The guest never signed in to Xbox Live in its window.
        let (n, why) = stopped(&in_lobby(), true);
        assert_eq!(n, 7);
        assert_eq!(
            why,
            "Bravo never opened the Xbox Live menu in their window, so Alpha had nobody online to invite"
        );
        let mut l = in_lobby();
        l.push(("Bravo", "custom games: row text active"));
        assert_eq!(
            stopped(&l, true).1,
            "Alpha saw its friends online but sent no invitation"
        );
        l.push((
            "Alpha",
            "coop-auto: invited Bravo to session 8000000000000001",
        ));
        assert!(stopped(&l, true).1.contains("Bravo never accepted"));
        l.push((
            "Bravo",
            "coop-auto: accepting Alpha's invitation to session 8000000000000001",
        ));
        let (n, why) = stopped(&l, true);
        assert_eq!(n, 9);
        assert!(why.contains("did not join Alpha's lobby"), "{why}");
        // Without --auto: accept by hand.
        let mut l = in_lobby();
        l.push(("Bravo", "custom games: row text active"));
        let (n, why) = stopped(&l, false);
        assert_eq!(n, 9);
        assert!(
            why.contains("accept Alpha's invitation in Bravo's friends list"),
            "{why}"
        );
        // In, pinged, but nobody pressed Start.
        l.push((
            "Bravo",
            "coop-live: guest: joined Alpha's co-op session 8000000000000001: Outskirts on Heroic",
        ));
        l.push(("Alpha", "coop-live: ping 3 ms"));
        l.push(("Bravo", "coop-live: ping 3 ms"));
        let (n, why) = stopped(&l, false);
        assert_eq!(n, 11);
        assert_eq!(why, "nobody pressed Start Game in Alpha's lobby");
        l.push(("Alpha", "coop-live: lobby start state 8"));
        assert!(stopped(&l, false).1.contains("all players ready?"));
    }

    #[test]
    fn a_game_that_never_signed_in() {
        let l = [("Alpha", "live: playing as Alpha (1) on x")];
        let (n, why) = stopped(&l, true);
        assert_eq!(n, 1);
        assert!(why.starts_with("Bravo never signed in"), "{why}");
    }

    /// The 2026-11-17 second run: Charlie's window was driven into a solo
    /// campaign; Alpha never left its main menu.
    #[test]
    fn a_guest_driven_down_the_hosts_path_is_named() {
        let roles = Roles {
            host: "Alpha".into(),
            guests: vec!["Bravo".into(), "Charlie".into()],
            late: vec![],
        };
        let mut text = roles.header();
        for (t, tag, line) in [
            (
                0.049,
                "Alpha",
                "live: playing as Alpha (0009d7e882ecd47d) on http://127.0.0.1:8075",
            ),
            (
                0.261,
                "Charlie",
                "live: playing as Charlie (000921eea626d1f4) on http://127.0.0.1:8075",
            ),
            (
                0.261,
                "Bravo",
                "live: playing as Bravo (0009f6de65cac40f) on http://127.0.0.1:8075",
            ),
            (1.302, "Charlie", "coop-live: 15 campaign levels (15 named)"),
            (1.343, "Alpha", "coop-live: 15 campaign levels (15 named)"),
            (1.550, "Bravo", "coop-live: 15 campaign levels (15 named)"),
            (265.962, "Charlie", "coop-live: lobby mode 4294967295"),
            (
                287.874,
                "Charlie",
                "skulls page: Charlie has no skulls collected; starting as retail",
            ),
        ] {
            text.push_str(&format_line(t, tag, line));
            text.push('\n');
        }
        let v = verdict(&text, true).unwrap();
        let s = v.stopped_at().unwrap();
        assert_eq!(s.n, 3);
        assert_eq!(
            s.why.as_deref(),
            Some(
                "you drove Charlie's window (it started a campaign or lobby itself); drive Alpha \
                 (host), the leftmost window, titled \"Halo 2 — Alpha (host)\""
            )
        );
        // Once the host walks its own path, the guest's walk is not the reason.
        text.push_str(&format_line(
            300.0,
            "Alpha",
            "custom games: row chosen active",
        ));
        text.push('\n');
        let v = verdict(&text, true).unwrap();
        assert!(
            v.stopped_at()
                .unwrap()
                .why
                .as_deref()
                .unwrap()
                .contains("picked another of its rows")
        );
    }

    #[test]
    fn a_late_guest_is_checked_for_its_join_not_the_lobby_steps() {
        let roles = Roles {
            host: "Alpha".into(),
            guests: vec!["Bravo".into()],
            late: vec!["Charlie".into(), "Delta".into(), "Echo".into()],
        };
        assert_eq!(Roles::read(&roles.header()), Some(roles.clone()));
        let mut text = roles.header();
        for (tag, line) in [
            (
                "Charlie",
                "coop-auto: accepting Alpha's invitation to session 80",
            ),
            (
                "Charlie",
                "coop-jip: joined in progress at tick 4322 as player 3",
            ),
            (
                "Alpha",
                "coop-jip: admitted player 3 at tick 4322 (900 KiB sent, held 840 ms)",
            ),
            (
                "Delta",
                "coop-auto: accepting Alpha's invitation to session 80",
            ),
            (
                "Delta",
                "coop-jip: back to the lobby: refused: the game is full",
            ),
            (
                "Echo",
                "coop-auto: accepting Alpha's invitation to session 80",
            ),
            ("Alpha", "coop-live: 15 campaign levels (15 named)"),
        ] {
            assert!(wanted(line), "{line}");
            text.push_str(&format_line(1.0, tag, line));
            text.push('\n');
        }
        let v = verdict(&text, true).unwrap();
        let says: Vec<(bool, &str)> = v
            .joins
            .iter()
            .map(|j| (j.joined, j.says.as_str()))
            .collect();
        assert_eq!(
            says,
            [
                (true, "Charlie joined in progress at tick 4322 (player 3)"),
                (
                    false,
                    "Delta did not join in progress: refused: the game is full"
                ),
                (
                    false,
                    "Echo accepted but never said it joined in progress (was Alpha in a level?)"
                ),
                (
                    true,
                    "Alpha admitted player 3 at tick 4322 (900 KiB sent, held 840 ms)"
                ),
            ]
        );
        // The late ones are not missing from the lobby's guest steps, but
        // they still have to read the levels like everyone.
        let joined = v
            .steps
            .iter()
            .find(|s| s.what.contains("the guest is in the host's"))
            .unwrap();
        assert_eq!(
            joined.seen,
            Seen::No {
                missing: vec!["Bravo".into()]
            }
        );
        let levels = v
            .steps
            .iter()
            .find(|s| s.what.contains("campaign levels"))
            .unwrap();
        assert!(
            matches!(&levels.seen, Seen::No { missing } if missing.contains(&"Echo".to_string())),
            "{:?}",
            levels.seen
        );
        let out = v.render();
        assert!(out.contains("join in progress (section 14):\n  [x] Charlie joined in progress at tick 4322 (player 3)"), "{out}");
        assert!(out.contains("  [ ] Delta did not join"), "{out}");
        assert!(v.trouble.iter().any(|t| t.contains("back to the lobby")));
        // No late guests, no section.
        let plain = Roles {
            host: "Alpha".into(),
            guests: vec!["Bravo".into()],
            late: vec![],
        };
        assert!(
            !verdict(&plain.header(), false)
                .unwrap()
                .render()
                .contains("join in progress")
        );
    }

    /// A full run of `n` games: game 0 hosts, the rest are guests. Every
    /// line a passing run prints, in order; each test then spoils one.
    fn run_of(n: usize) -> (Roles, Vec<(String, String)>) {
        let tags: Vec<String> = (0..n).map(|i| format!("G{i:02}")).collect();
        let roles = Roles {
            host: tags[0].clone(),
            guests: tags[1..].to_vec(),
            late: vec![],
        };
        let host = tags[0].clone();
        let mut l: Vec<(String, String)> = Vec::new();
        let mut add = |t: &str, s: String| l.push((t.to_string(), s));
        for t in &tags {
            add(t, format!("live: playing as {t} (1) on x"));
            add(t, "coop-live: 15 campaign levels (15 named)".into());
        }
        add(&host, "coop-live: Live menu row 5 in co-op mode".into());
        add(
            &host,
            "coop-live: hosting a co-op campaign in the game's own lobby".into(),
        );
        add(
            &host,
            "coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)"
                .into(),
        );
        add(
            &host,
            "coop-live: the lobby holds Outskirts on Heroic; advertised".into(),
        );
        for g in &tags[1..] {
            add(&host, format!("coop-auto: invited {g} to session 80"));
            add(g, "coop-auto: accepting G00's invitation to session 80".into());
            add(g, "coop-live: guest: joined G00's co-op session 80: x".into());
        }
        for t in &tags {
            add(t, "coop-live: ping 3 ms".into());
        }
        add(
            &host,
            format!("coop-live: lobby start state 7 (co-op wants one console) answered 9 for {n} consoles"),
        );
        add(
            &host,
            format!("coop-probe: session start: campaign with {n} peers: showing the start 1 peer until OPTIONS_NEW"),
        );
        for t in &tags {
            add(t, "coop-probe: session start returned 1".into());
        }
        add(&host, "coop-probe: simulation mode sync-server (2) at tick 0".into());
        for g in &tags[1..] {
            add(g, "coop-probe: simulation mode sync-client (3) at tick 0".into());
        }
        for t in &tags {
            add(t, "coop-probe: checkpoint save at tick 90".into());
        }
        for i in 2..n {
            add(
                &host,
                format!("coop-players: player {i} placed at starting location {}, ring 1 (0 0 0)", i % 2),
            );
        }
        (roles, l)
    }

    fn text_of(roles: &Roles, lines: &[(String, String)]) -> String {
        let mut s = roles.header();
        for (i, (t, x)) in lines.iter().enumerate() {
            s.push_str(&format_line(i as f64, t, x));
            s.push('\n');
        }
        s
    }

    fn failing(roles: &Roles, lines: &[(String, String)]) -> Vec<usize> {
        let v = verdict(&text_of(roles, lines), true).unwrap();
        v.steps
            .iter()
            .filter(|s| s.seen != Seen::Yes)
            .map(|s| s.n)
            .collect()
    }

    #[test]
    fn full_runs_of_2_and_16_pass_every_step() {
        for n in [2, 16] {
            let (roles, l) = run_of(n);
            assert_eq!(failing(&roles, &l), Vec::<usize>::new(), "n = {n}");
        }
    }

    #[test]
    fn step_7_wants_an_invitation_per_guest() {
        let (roles, mut l) = run_of(16);
        // One invitation survives: the old check passed on it.
        let keep = l
            .iter()
            .position(|(_, x)| x.starts_with("coop-auto: invited "))
            .unwrap();
        let mut i = 0;
        l.retain(|(_, x)| {
            let drop = x.starts_with("coop-auto: invited ") && i != keep;
            i += 1;
            !drop
        });
        assert_eq!(failing(&roles, &l), vec![7]);
        let (roles, mut l) = run_of(2);
        l.retain(|(_, x)| !x.starts_with("coop-auto: invited "));
        assert_eq!(failing(&roles, &l), vec![7]);
    }

    #[test]
    fn steps_11_and_12_want_the_consoles_and_peers_to_be_n() {
        let (roles, mut l) = run_of(16);
        for (_, x) in l.iter_mut() {
            *x = x.replace("for 16 consoles", "for 2 consoles");
        }
        assert_eq!(failing(&roles, &l), vec![11]);
        let (roles, mut l) = run_of(16);
        for (_, x) in l.iter_mut() {
            *x = x.replace("campaign with 16 peers", "campaign with 9 peers");
        }
        assert_eq!(failing(&roles, &l), vec![12]);
        let (roles, mut l) = run_of(2);
        for (_, x) in l.iter_mut() {
            *x = x.replace("for 2 consoles", "for 1 consoles");
        }
        assert_eq!(failing(&roles, &l), vec![11]);
    }

    #[test]
    fn step_13_wants_every_game_to_start_the_session() {
        let (roles, mut l) = run_of(16);
        let at = l
            .iter()
            .rposition(|(_, x)| x == "coop-probe: session start returned 1")
            .unwrap();
        l.remove(at);
        assert_eq!(failing(&roles, &l), vec![13]);
    }

    #[test]
    fn step_16_wants_one_checkpoint_tick() {
        let (roles, mut l) = run_of(16);
        let at = l
            .iter()
            .rposition(|(_, x)| x.contains("checkpoint save"))
            .unwrap();
        l[at].1 = "coop-probe: checkpoint save at tick 91".into();
        let v = verdict(&text_of(&roles, &l), true).unwrap();
        assert_eq!(
            v.steps[15].seen,
            Seen::No {
                missing: vec!["G15 (tick 91, not 90)".into()]
            }
        );
        l.remove(at);
        assert_eq!(failing(&roles, &l), vec![16]);
    }

    #[test]
    fn step_17_wants_players_3_to_n_placed_and_none_refused() {
        let (roles, mut l) = run_of(16);
        let at = l
            .iter()
            .position(|(_, x)| x.starts_with("coop-players: player 7 "))
            .unwrap();
        l.remove(at);
        assert_eq!(failing(&roles, &l), vec![17]);
        l.insert(
            at,
            (
                "G00".into(),
                "coop-players: player 7: REFUSED: no free spot in any ring"
                    .into(),
            ),
        );
        assert_eq!(failing(&roles, &l), vec![17]);
        let (roles, mut l) = run_of(2);
        assert_eq!(failing(&roles, &l), Vec::<usize>::new());
        l.push((
            "G01".into(),
            "coop-players: player 2: REFUSED: no free spot in any ring"
                .into(),
        ));
        assert_eq!(failing(&roles, &l), vec![17]);
    }

    /// `--solo-start` with `n` games: G00 starts alone, the rest join in
    /// progress. Every line a passing run prints.
    fn solo_run_of(n: usize) -> (Roles, Vec<(String, String)>) {
        let tags: Vec<String> = (0..n).map(|i| format!("G{i:02}")).collect();
        let roles = Roles {
            host: tags[0].clone(),
            guests: vec![],
            late: tags[1..].to_vec(),
        };
        let host = tags[0].clone();
        let mut l: Vec<(String, String)> = Vec::new();
        let mut add = |t: &str, s: String| l.push((t.to_string(), s));
        for t in &tags {
            add(t, format!("live: playing as {t} (1) on x"));
            add(t, "coop-live: 15 campaign levels (15 named)".into());
        }
        add(&host, "coop-live: Live menu row 5 in co-op mode".into());
        add(
            &host,
            "coop-live: hosting a co-op campaign in the game's own lobby".into(),
        );
        add(
            &host,
            "coop-live: the Co-op Campaign row's lobby: mode 2 (custom game) set as 0 (co-op campaign)"
                .into(),
        );
        add(
            &host,
            "coop-live: the lobby holds Outskirts on Heroic; advertised".into(),
        );
        for g in &tags[1..] {
            add(&host, format!("coop-auto: invited {g} to session 80"));
        }
        add(
            &host,
            "coop-live: lobby start state 7 (co-op wants one console) answered 9 for 1 console (alone; friends join in progress)".into(),
        );
        add(
            &host,
            "coop-probe: session start: campaign with 1 peer (alone): showing the start 2 players until OPTIONS_NEW, so the game is co-op; friends join in progress".into(),
        );
        add(&host, "coop-probe: session start returned 1".into());
        add(&host, "coop-probe: simulation mode sync-server (2) at tick 0".into());
        add(&host, "coop-probe: checkpoint save at tick 90".into());
        for (i, g) in tags[1..].iter().enumerate() {
            add(g, "coop-auto: accepting G00's invitation to session 80".into());
            add(
                &host,
                "coop-jip: host: a console joins the running game (session state 7 read as the lobby's for the join check)".into(),
            );
            add(g, "coop-probe: simulation mode sync-client (3) at tick 4000".into());
            add(g, format!("coop-jip: joined in progress at tick 4000 as player {}", i + 2));
            add(
                &host,
                format!("coop-players: player {} joins in progress: placed next to player 0 (the nearest the squad's middle), ring 1 (1 2 3)", i + 1),
            );
        }
        for t in &tags {
            add(t, "coop-live: ping 3 ms".into());
        }
        (roles, l)
    }

    #[test]
    fn a_solo_start_passes_its_own_steps_and_the_lobby_steps() {
        for n in [2, 4] {
            let (roles, l) = solo_run_of(n);
            assert_eq!(failing(&roles, &l), Vec::<usize>::new(), "n = {n}");
            let v = verdict(&text_of(&roles, &l), true).unwrap();
            assert_eq!(v.solo.len(), 6);
            assert!(v.solo.iter().all(|j| j.joined), "{:?}", v.solo);
            assert!(v.joins.iter().filter(|j| j.tag != roles.host).all(|j| j.joined));
            let out = v.render();
            assert!(out.contains("solo start (live-coop-join.md 13.4):\n  [x] G00 started alone"), "{out}");
        }
        // A lobby run has no solo part.
        let (roles, l) = run_of(2);
        assert!(verdict(&text_of(&roles, &l), true).unwrap().solo.is_empty());
    }

    #[test]
    fn a_solo_start_fails_on_a_revert_a_misplaced_newcomer_or_a_closed_game() {
        let solo = |l: &[(String, String)], roles: &Roles| {
            verdict(&text_of(roles, l), true)
                .unwrap()
                .solo
                .into_iter()
                .filter(|j| !j.joined)
                .map(|j| j.says)
                .collect::<Vec<_>>()
        };
        let (roles, mut l) = solo_run_of(3);
        l.push(("G01".into(), "coop-probe: checkpoint revert at tick 4100".into()));
        assert_eq!(solo(&l, &roles), ["reverted after a join: G01"]);
        let (roles, mut l) = solo_run_of(3);
        l.retain(|(_, x)| !x.contains("player 2 joins in progress"));
        l.push((
            "G00".into(),
            "coop-players: player 2 joins in progress: REFUSED: no free spot".into(),
        ));
        assert_eq!(
            solo(&l, &roles),
            ["1 of 2 newcomer(s) placed next to the squad; 1 REFUSED"]
        );
        let (roles, mut l) = solo_run_of(3);
        l.retain(|(_, x)| !x.contains("a console joins the running game"));
        let bad = solo(&l, &roles);
        assert!(bad.contains(&"G00 stayed joinable: let 0 of 2 console(s) into the running game".to_string()), "{bad:?}");
        assert!(bad.contains(&"no join to watch for reverts".to_string()), "{bad:?}");
        // The lobby kept the co-op check: never started alone.
        let (roles, mut l) = solo_run_of(2);
        for (_, x) in l.iter_mut() {
            if x.contains("answered 9 for 1 console") {
                *x = "coop-live: lobby start state 7".into();
            }
        }
        let bad = solo(&l, &roles);
        assert!(bad[0].contains("kept the co-op check"), "{bad:?}");
    }

    #[test]
    fn out_of_sync_fails_the_run_and_the_filter_keeps_the_new_lines() {
        let (roles, mut l) = run_of(2);
        l.push((
            "G01".into(),
            "OUT OF SYNC: the retail latch (simulation globals +0x2c) is set at tick 12, update 77"
                .into(),
        ));
        assert_eq!(failing(&roles, &l), vec![18]);
        let v = verdict(&text_of(&roles, &l), true).unwrap();
        assert_eq!(v.trouble.len(), 1);
        for line in [
            "coop-players: player 3 placed next to the anchor",
            "coop-probe: checkpoint save at tick 90",
            "OUT OF SYNC: the retail latch (simulation globals +0x2c) is set",
        ] {
            assert!(wanted(line), "{line}");
        }
    }
}
