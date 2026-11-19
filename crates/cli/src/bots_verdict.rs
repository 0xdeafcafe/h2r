//! The bots test's log and verdict (`h2 bots test`, `h2 bots verdict`;
//! docs/game/bots.md, "Testing"): which game lines go into the merged
//! log, and which checks a log shows passed, in the order a run reaches
//! them, each with the lines that explain it.
//!
//! A merged log starts with `# host <gamertag>`, then one line per game
//! line, `[+SECONDS] GAMERTAG | TEXT` (the co-op test's format,
//! [`crate::coop_verdict::format_line`]). The harness adds its own lines:
//! `bots-test: plan: ...` (what it asked for), `bots-test: status ...`
//! (the agent API's phase, mode and map when they change),
//! `bots-test: session ...` (the Live server's session for the host),
//! `bots-test: game ...` (the server's game record) and `bots-drive: ...`
//! (each menu press, [`crate::bots_drive`]).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::coop_verdict::parse_line;

/// Local bots play on ports 1 to 3; the rest of the count are phantoms.
pub const LOCAL_PORTS: usize = 3;

/// Whether a game's stderr line goes into the merged log.
pub fn wanted(line: &str) -> bool {
    const KEEP: [&str; 15] = [
        "needs a code to sign in",
        "live: sign-in",
        "bots:",
        "clankers:",
        "live: playing as",
        "live: session",
        "coop-live: lobby mode",
        "custom games",
        "map select",
        "agent: listening",
        "ui: screen",
        "panicked",
        "guest thread exited",
        "bots-drive",
        "bots-test",
    ];
    KEEP.iter().any(|k| line.contains(k))
}

/// What a run asked for: its first `bots-test: plan:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub count: usize,
    pub tag: String,
    /// The map the game must be on; any when `None`.
    pub map: Option<String>,
}

const PLAN: &str = "bots-test: plan: ";

impl Plan {
    pub fn local(&self) -> usize {
        self.count.min(LOCAL_PORTS)
    }

    pub fn phantoms(&self) -> usize {
        self.count - self.local()
    }

    /// `bots-test: plan: count 5 (3 local, 2 phantoms), gamertag Tester, map any`
    pub fn line(&self) -> String {
        format!(
            "{PLAN}count {} ({} local, {} phantoms), gamertag {}, map {}",
            self.count,
            self.local(),
            self.phantoms(),
            self.tag,
            self.map.as_deref().unwrap_or("any")
        )
    }

    /// The plan a log's plan line names.
    pub fn read(texts: &[&str]) -> Option<Plan> {
        let rest = texts.iter().find_map(|t| t.strip_prefix(PLAN))?;
        let count = rest
            .strip_prefix("count ")?
            .split(' ')
            .next()?
            .parse()
            .ok()?;
        let tag = rest
            .split_once(", gamertag ")?
            .1
            .split(", map ")
            .next()?
            .to_string();
        let map = rest
            .rsplit_once(", map ")
            .map(|(_, m)| m.trim())
            .filter(|m| *m != "any")
            .map(str::to_string);
        Some(Plan { count, tag, map })
    }
}

/// One check of the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub pass: bool,
    /// What was seen, or what is missing and why.
    pub detail: String,
    /// The log lines (merged, with their times) that show it.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub plan: Plan,
    pub checks: Vec<Check>,
    /// Lines that look like trouble anywhere in the run.
    pub trouble: Vec<String>,
}

/// Lines (merged) and their texts.
struct Log<'a> {
    raw: Vec<&'a str>,
    texts: Vec<&'a str>,
}

impl<'a> Log<'a> {
    fn new(log: &'a str) -> Self {
        let mut raw = Vec::new();
        let mut texts = Vec::new();
        for l in log.lines() {
            if let Some((_, text)) = parse_line(l) {
                raw.push(l);
                texts.push(text);
            }
        }
        Log { raw, texts }
    }

    /// The merged lines whose text `f` keeps, at most `n`, the last ones.
    fn last(&self, n: usize, f: impl Fn(&str) -> bool) -> Vec<String> {
        let mut v: Vec<String> = self
            .raw
            .iter()
            .zip(&self.texts)
            .filter(|(_, t)| f(t))
            .map(|(r, _)| r.to_string())
            .collect();
        let skip = v.len().saturating_sub(n);
        v.drain(..skip);
        v
    }

    /// The first merged line whose text `f` keeps.
    fn first(&self, f: impl Fn(&str) -> bool) -> Option<(String, &'a str)> {
        self.raw
            .iter()
            .zip(&self.texts)
            .find(|(_, t)| f(t))
            .map(|(r, t)| (r.to_string(), *t))
    }
}

fn check(name: &str, pass: bool, detail: String, lines: Vec<String>) -> Check {
    Check {
        name: name.into(),
        pass,
        detail,
        lines,
    }
}

/// `bots: join gate port P: ...` and `bots: signing NAME in on port P ...`:
/// the port.
fn gate_port(text: &str) -> Option<u8> {
    text.strip_prefix("bots: join gate port ")?
        .split(':')
        .next()?
        .parse()
        .ok()
}

fn signing_port(text: &str) -> Option<u8> {
    let rest = text.strip_prefix("bots: signing ")?;
    let (_, after) = rest.rsplit_once(" on port ")?;
    after.split(' ').next()?.parse().ok()
}

/// `clankers: Clanker P is GAMERTAG on Xbox Live (bot account XUID)`: the
/// port, gamertag and xuid.
fn account_line(text: &str) -> Option<(u8, &str, &str)> {
    let rest = text.strip_prefix("clankers: Clanker ")?;
    let (port, rest) = rest.split_once(" is ")?;
    let (tag, rest) = rest.split_once(" on Xbox Live (bot account ")?;
    Some((port.parse().ok()?, tag, rest.strip_suffix(')')?))
}

/// `bots: NAME on port P is on Xbox Live as its own bot account, ...`.
fn on_live_as_account(text: &str) -> Option<u8> {
    let rest = text.strip_prefix("bots: ")?;
    let (_, after) = rest.split_once(" on port ")?;
    let (port, after) = after.split_once(' ')?;
    after
        .starts_with("is on Xbox Live as its own bot account")
        .then(|| port.parse::<u8>().ok())?
}

/// `bots: phantom NAME joins (peer N)`: the name.
fn phantom_joined(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("bots: phantom ")?;
    let (name, after) = rest.rsplit_once(" joins (peer ")?;
    after.ends_with(')').then_some(name)
}

/// A phantom that could not be made: a `failed` line about a phantom, or
/// the driver's `clankers: NAME (phantom...): ERROR`.
fn phantom_failed(text: &str) -> bool {
    (text.contains("phantom") && text.contains("failed"))
        || (text.starts_with("clankers: ")
            && text
                .split_once("(phantom")
                .is_some_and(|(_, r)| r.contains("): ")))
}

/// `bots-test: status phase P mode M map X`: phase, mode, map.
pub fn status_line(phase: &str, mode: &str, map: &str) -> String {
    format!("bots-test: status phase {phase} mode {mode} map {map}")
}

fn read_status(text: &str) -> Option<(&str, &str, &str)> {
    let rest = text.strip_prefix("bots-test: status phase ")?;
    let (phase, rest) = rest.split_once(" mode ")?;
    let (mode, map) = rest.split_once(" map ")?;
    Some((phase, mode, map))
}

/// `bots-test: session ID: F filled (public A, private B), J joined`.
pub fn session_line(id: &str, public: u64, private: u64, joined: usize) -> String {
    format!(
        "bots-test: session {id}: {} filled (public {public}, private {private}), {joined} joined",
        public + private
    )
}

fn read_session(text: &str) -> Option<u64> {
    let rest = text.strip_prefix("bots-test: session ")?;
    let (_, rest) = rest.split_once(": ")?;
    rest.split(' ').next()?.parse().ok()
}

/// `bots-test: game ID on MAP: P players, B bots (A with an account)`.
pub fn game_line(id: &str, map: &str, players: usize, bots: usize, accounts: usize) -> String {
    format!(
        "bots-test: game {id} on {map}: {players} players, {bots} bots ({accounts} with an account)"
    )
}

fn read_game(text: &str) -> Option<(usize, usize)> {
    let rest = text.strip_prefix("bots-test: game ")?;
    let (_, rest) = rest.split_once(": ")?;
    let (_, rest) = rest.split_once(" players, ")?;
    let bots = rest.split(' ').next()?.parse().ok()?;
    let accounts = rest.split_once('(')?.1.split(' ').next()?.parse().ok()?;
    Some((bots, accounts))
}

/// A hosted Live session: the HLE's `live: session ... advertised`.
fn is_advertised(text: &str) -> bool {
    text.starts_with("live: session ") && text.contains(" advertised")
}

/// The players' screen mode a run ended in when it was not Xbox Live: the
/// last `ui: screen 0x1e ..., mode N` with N not 3, unless the Xbox Live
/// menu or a Live session came after it. Its name and line.
fn ended_off_live(l: &Log) -> Option<(&'static str, String)> {
    let mut ended = None;
    for (raw, t) in l.raw.iter().zip(&l.texts) {
        if let Some(mode) = t
            .strip_prefix("ui: screen 0x1e")
            .and_then(|r| r.rsplit_once(", mode "))
            .and_then(|(_, m)| m.trim().parse::<u32>().ok())
        {
            ended = match mode {
                1 => Some(("Split Screen", raw.to_string())),
                2 => Some(("System Link", raw.to_string())),
                _ => None,
            };
        } else if t.starts_with("ui: screen 0xba") || is_advertised(t) {
            ended = None;
        }
    }
    ended
}

/// The account picker opening, as port 1's join gate names it (one line
/// per change, so one per opening).
fn picker_opened(text: &str) -> bool {
    text.starts_with("bots: join gate port 1: ") && text.contains(", dialog 0x14 (")
}

const TROUBLE: [&str; 7] = [
    "panicked",
    "guest thread exited",
    "fault at",
    "bots-drive: stuck",
    "refused",
    "no roster from Live",
    "bots-test: timed out",
];

/// The verdict a merged log shows; `None` without a plan line.
pub fn verdict(log: &str) -> Option<Verdict> {
    let l = Log::new(log);
    let plan = Plan::read(&l.texts)?;
    let local = plan.local() as u8;
    let mut checks = Vec::new();

    // 1. The game signed in to Live.
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
            l.last(3, |t| t.starts_with("agent: listening")),
        ),
    });

    // 2. The Clankers driver took the settings: one local bot per port.
    let on: BTreeSet<u8> = l
        .texts
        .iter()
        .filter_map(|t| {
            let rest = t.strip_prefix("clankers: Clanker ")?;
            let (port, after) = rest.split_once(' ')?;
            after.starts_with("on (").then(|| port.parse::<u8>().ok())?
        })
        .collect();
    let missing: Vec<u8> = (1..=local).filter(|p| !on.contains(p)).collect();
    checks.push(check(
        "Clankers on (the settings reached the driver)",
        missing.is_empty(),
        if missing.is_empty() {
            format!("{} local bots on ports 1 to {local}", on.len())
        } else {
            format!(
                "no `clankers: Clanker N on` for port{} {:?}: clankers.enabled/clankers.count \
                 did not reach the driver, or the port was taken",
                if missing.len() == 1 { "" } else { "s" },
                missing
            )
        },
        l.last(6, |t| t.starts_with("clankers: ")),
    ));

    // 3. On Xbox Live: the players' screen in Xbox Live mode, or the Xbox
    // Live menu. A run that ended in another mode says which.
    let ended = ended_off_live(&l);
    let live = l.first(|t| {
        t.starts_with("ui: screen 0xba")
            || t.starts_with("custom games: xbox live menu tag")
            || (t.starts_with("ui: screen 0x1e") && t.ends_with(", mode 3"))
    });
    checks.push(match (live, &ended) {
        (Some((line, t)), None) => check("on Xbox Live", true, t.to_string(), vec![line]),
        (_, Some((mode, line))) => check(
            "on Xbox Live",
            false,
            format!("not on Xbox Live: ended in {mode}"),
            vec![line.clone()],
        ),
        (None, None) => {
            let mut lines = l.last(5, |t| t.starts_with("bots-drive"));
            lines.extend(l.last(1, |t| t.starts_with("ui: screen")));
            let pickers = l.texts.iter().filter(|t| picker_opened(t)).count();
            let detail = if pickers > 0 {
                format!(
                    "never on Xbox Live: the game's account picker (dialog 0x14, \
                     `dialog_live_signin`) opened {pickers} time(s) and the profile never signed \
                     in (A on its row 0 is \"no account\"; the run's account is row 1); the last \
                     presses and screen follow"
                )
            } else {
                "never on Xbox Live (no players' screen in mode 3, no Xbox Live menu `0xba`): \
                 the last presses and screen follow"
                    .into()
            };
            check("on Xbox Live", false, detail, lines)
        }
    });

    // 4. Hosting a Live custom game: the HLE advertised the session. A
    // lobby (`coop-live: lobby mode 2`) alone is any custom game, Split
    // Screen and System Link too.
    let hosting = l.first(is_advertised);
    checks.push(match hosting {
        Some((line, t)) => check(
            "hosting a Live custom game (session advertised)",
            true,
            t.to_string(),
            vec![line],
        ),
        None => {
            let lobby = l.first(|t| t.starts_with("coop-live: lobby mode 2"));
            let mut lines = l.last(5, |t| t.starts_with("bots-drive"));
            lines.extend(l.last(1, |t| t.starts_with("ui: screen")));
            let detail = match (&lobby, &ended) {
                (Some(_), Some((mode, _))) => format!(
                    "a custom game lobby opened in {mode}, not on Xbox Live: no \
                     `live: session ... advertised`"
                ),
                (Some((l2, _)), None) => {
                    lines.insert(0, l2.clone());
                    "a custom game lobby opened (lobby mode 2) but the HLE advertised no Live \
                     session: it is not a Live game"
                        .into()
                }
                (None, _) => "no `live: session ... advertised`: the menus never reached Xbox \
                              Live > Custom Game; the last presses and screen follow"
                    .into(),
            };
            check(
                "hosting a Live custom game (session advertised)",
                false,
                detail,
                lines,
            )
        }
    });

    // 5. The roster: the server's bot accounts for the whole count.
    let asked =
        l.first(|t| t.starts_with(&format!("clankers: asking Live for {} bots", plan.count)));
    let rostered = l.first(|t| t.starts_with("clankers: Live rostered bots for session "));
    checks.push(match (&asked, &rostered) {
        (Some(_), Some((line, t))) => check(
            "Live roster of bot accounts",
            true,
            t["clankers: ".len()..].to_string(),
            vec![asked.clone().unwrap().0, line.clone()],
        ),
        _ => check(
            "Live roster of bot accounts",
            false,
            match &asked {
                None => format!(
                    "never asked: no `clankers: asking Live for {} bots for session ...` (the \
                     driver saw no hosted Live session)",
                    plan.count
                ),
                Some(_) => "asked, but no `clankers: Live rostered bots`: the server refused or \
                            never answered"
                    .into(),
            },
            l.last(4, |t| {
                t.contains("roster") || t.starts_with("clankers: asking Live")
            }),
        ),
    });

    // 6. Each local bot signed in through the join gate.
    let mut where_ = Vec::new();
    let mut lines = Vec::new();
    let mut missing = Vec::new();
    for p in 1..=local {
        let signs = l.first(|t| gate_port(t) == Some(p) && t.ends_with(": signs in"));
        let signed = l.first(|t| signing_port(t) == Some(p));
        match (signs, signed) {
            (Some((g, gt)), Some((s, st))) => {
                let at = if st.contains("in the pregame lobby") || gt.contains("screen lobby") {
                    "the lobby"
                } else if st.contains("on the players screen") {
                    "the players screen"
                } else {
                    "elsewhere"
                };
                where_.push(format!("port {p} in {at}"));
                lines.push(g);
                lines.push(s);
            }
            _ => {
                missing.push(p);
                lines.extend(l.last(2, |t| gate_port(t) == Some(p)));
            }
        }
    }
    checks.push(check(
        "local bots signed in (join gate)",
        missing.is_empty(),
        if missing.is_empty() {
            where_.join(", ")
        } else {
            format!(
                "port{} {:?} never signed in; the last join gate lines say what it waited on \
                 (screen, dialog, signed_in)",
                if missing.len() == 1 { "" } else { "s" },
                missing
            )
        },
        lines,
    ));

    // 7. Each local bot on Live as its own bot account.
    let mut lines = Vec::new();
    let mut missing = Vec::new();
    let mut names = Vec::new();
    for p in 1..=local {
        let account = l.first(|t| {
            account_line(t).is_some_and(|(port, _, xuid)| {
                port == p && xuid.to_ascii_lowercase().starts_with("000b")
            })
        });
        let live = l.first(|t| on_live_as_account(t) == Some(p));
        match (account, live) {
            (Some((a, at)), Some((b, _))) => {
                if let Some((_, tag, xuid)) = account_line(at) {
                    names.push(format!("{tag} ({xuid})"));
                }
                lines.push(a);
                lines.push(b);
            }
            _ => {
                missing.push(p);
                lines.extend(l.last(2, |t| {
                    t.contains(&format!(" on port {p} "))
                        || t.starts_with(&format!("clankers: Clanker {p} "))
                }));
            }
        }
    }
    checks.push(check(
        "local bots on Xbox Live as bot accounts (000b...)",
        missing.is_empty(),
        if missing.is_empty() {
            names.join(", ")
        } else {
            format!(
                "port{} {:?}: no bot account sign-in (`Clanker N is ... (bot account 000b...)` \
                 and `... is on Xbox Live as its own bot account`); a guest or offline line \
                 says why",
                if missing.len() == 1 { "" } else { "s" },
                missing
            )
        },
        lines,
    ));

    // 8. Start Game with the bots in: the start state.
    let ready =
        l.first(|t| t.starts_with("bots: start state 9") || t.starts_with("bots: start state 10"));
    checks.push(match ready {
        Some((line, t)) => check("Start Game: start state ready", true, t.into(), vec![line]),
        None => {
            let states = l.last(3, |t| t.starts_with("bots: start state"));
            let detail = if states.is_empty() {
                "no `bots: start state` line: Start Game was never pressed with the bots in \
                 the lobby"
                    .to_string()
            } else {
                "Start Game was pressed but the lobby would not start; the start state says \
                 which check held it"
                    .to_string()
            };
            let mut lines = states;
            lines.extend(l.last(2, |t| t.starts_with("bots-drive")));
            check("Start Game: start state ready", false, detail, lines)
        }
    });

    // 9. Gameplay on a map.
    let play = l.first(|t| {
        read_status(t).is_some_and(|(phase, mode, map)| {
            phase == "gameplay"
                && mode != "campaign"
                && plan.map.as_deref().is_none_or(|want| map == want)
        })
    });
    checks.push(match play {
        Some((line, t)) => check(
            "gameplay on a map",
            true,
            read_status(t)
                .map(|s| format!("on {}", s.2))
                .unwrap_or_default(),
            vec![line],
        ),
        None => {
            let other = l.first(|t| read_status(t).is_some_and(|s| s.0 == "gameplay"));
            let detail = match (&plan.map, other) {
                (Some(want), Some((_, t))) => format!(
                    "the game played {}, not {want} (--map checks the map; pick it in the lobby)",
                    read_status(t).map_or("?", |s| s.2)
                ),
                _ => "the agent API never answered phase gameplay in a multiplayer game".into(),
            };
            check(
                "gameplay on a map",
                false,
                detail,
                l.last(3, |t| t.starts_with("bots-test: status")),
            )
        }
    });

    // 10. The phantoms joined.
    let want = plan.phantoms();
    let joined: BTreeSet<&str> = l.texts.iter().filter_map(|t| phantom_joined(t)).collect();
    let failed = l.last(3, phantom_failed);
    checks.push(if want == 0 {
        check(
            "phantoms joined",
            failed.is_empty(),
            "none asked for (count 3 or less)".into(),
            failed,
        )
    } else {
        let pass = joined.len() >= want && failed.is_empty();
        let mut lines = l.last(want, |t| phantom_joined(t).is_some());
        lines.extend(failed.iter().cloned());
        if !pass {
            lines.extend(l.last(3, |t| {
                t.starts_with("bots: phantom ") && t.contains(" waits")
            }));
        }
        check(
            "phantoms joined",
            pass,
            if pass {
                format!(
                    "{} joined: {}",
                    joined.len(),
                    Vec::from_iter(joined).join(", ")
                )
            } else if !failed.is_empty() {
                "a phantom failed".into()
            } else {
                format!(
                    "{} of {want} joined (`bots: phantom NAME joins (peer N)`); they join once \
                     the game this console hosts runs, so a wait line says why not",
                    joined.len()
                )
            },
            lines,
        )
    });

    // 11. The session's player count reached the host and every bot.
    let most = l.texts.iter().filter_map(|t| read_session(t)).max();
    let need = 1 + plan.count as u64;
    checks.push(check(
        "the Live session holds 1 + count players",
        most.is_some_and(|m| m >= need),
        match most {
            Some(m) if m >= need => format!("{m} filled (needs {need})"),
            Some(m) => format!(
                "at most {m} filled of the {need} wanted: the session never counted every bot"
            ),
            None => "the Live server never listed a session for the host".into(),
        },
        l.last(2, |t| t.starts_with("bots-test: session ")),
    ));

    // 12. Telemetry: the game record has the bots.
    let best = l.texts.iter().filter_map(|t| read_game(t)).max();
    checks.push(check(
        "game record: the bots with bot: true",
        best.is_some_and(|(b, _)| b >= plan.count),
        match best {
            Some((b, a)) if b >= plan.count => format!("{b} bots, {a} with a bot account"),
            Some((b, _)) => format!("{b} bots on the record of the {} asked for", plan.count),
            None => "the Live server has no game record from this console".into(),
        },
        l.last(1, |t| t.starts_with("bots-test: game ")),
    ));

    let trouble = l.last(8, |t| TROUBLE.iter().any(|k| t.contains(*k)));
    Some(Verdict {
        plan,
        checks,
        trouble,
    })
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|c| c.pass)
    }

    /// The first check that failed, and its number (from 1).
    pub fn first_failure(&self) -> Option<(usize, &Check)> {
        self.checks
            .iter()
            .enumerate()
            .find(|(_, c)| !c.pass)
            .map(|(i, c)| (i + 1, c))
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        let p = &self.plan;
        let _ = writeln!(
            s,
            "bots test: {} Clankers ({} local, {} phantoms) with {} on a Live custom game",
            p.count,
            p.local(),
            p.phantoms(),
            p.tag
        );
        for (i, c) in self.checks.iter().enumerate() {
            let mark = if c.pass { "pass" } else { "FAIL" };
            let _ = writeln!(s, "{:>2}. [{mark}] {}: {}", i + 1, c.name, c.detail);
            for l in &c.lines {
                let _ = writeln!(s, "        {l}");
            }
        }
        if !self.trouble.is_empty() {
            let _ = writeln!(s, "trouble:");
            for l in &self.trouble {
                let _ = writeln!(s, "        {l}");
            }
        }
        match self.first_failure() {
            None => {
                let _ = writeln!(s, "verdict: PASS");
            }
            Some((n, c)) => {
                let _ = writeln!(s, "verdict: FAIL, first at step {n} ({})", c.name);
            }
        }
        s
    }
}

#[cfg(test)]
mod tests;
