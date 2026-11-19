//! The co-op test's log and checklist (`h2 coop test`, `h2 coop verdict`;
//! docs/live/live-coop.md section 12): which lines of each game go into the
//! merged log, how a merged line reads, and which checklist steps a log
//! shows.
//!
//! A merged log starts with one `# host <gamertag>` line, a
//! `# guest <gamertag>` line per guest and a `# late <gamertag>` line per
//! guest that joins once the host is in the level (`--late`), then one line
//! per game line: `[+SECONDS] GAMERTAG | TEXT`.

use std::fmt::Write as _;

/// Whether a game's stderr line goes into the merged log: the co-op
/// lines, not the boot-time dumps that merely name a co-op hook (the
/// feature plan, a hook's first `active`, the health model's `game start`).
pub fn wanted(line: &str) -> bool {
    const DROP: [&str; 3] = ["feature plan:", "feature: ", "health: "];
    const DROP_ANY: [&str; 1] = ["hooks installed via feature plan"];
    const KEEP: [&str; 17] = [
        "coop-live",
        "coop-jip",
        "coop-probe",
        "coop-auto",
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
    Everyone,
}

/// A checklist step: seen when every game it is for printed a line holding
/// all of `all`. `auto` steps only print with `--auto`.
struct Step {
    what: &'static str,
    who: Who,
    all: &'static [&'static str],
    auto: bool,
}

/// docs/live/live-coop.md sections 11.3 and 9.2, in the order a run passes
/// them.
const STEPS: &[Step] = &[
    Step {
        what: "signed in to Live",
        who: Who::Everyone,
        all: &["live: playing as "],
        auto: false,
    },
    Step {
        what: "the disc's campaign levels read",
        who: Who::Everyone,
        all: &["coop-live: ", " campaign levels ("],
        auto: false,
    },
    Step {
        what: "Co-op Campaign opens the game's own lobby",
        who: Who::Host,
        all: &["coop-live: Live menu row ", "in co-op mode"],
        auto: false,
    },
    Step {
        what: "hosting a co-op campaign in that lobby",
        who: Who::Host,
        all: &["coop-live: hosting a co-op campaign in the game's own lobby"],
        auto: false,
    },
    Step {
        what: "the lobby is in co-op mode",
        who: Who::Host,
        all: &[
            "coop-live: the Co-op Campaign row's lobby: mode ",
            "(co-op campaign)",
        ],
        auto: false,
    },
    Step {
        what: "level and difficulty advertised",
        who: Who::Host,
        all: &["coop-live: the lobby holds ", "; advertised"],
        auto: false,
    },
    Step {
        what: "the host invited the guest",
        who: Who::Host,
        all: &["coop-auto: invited "],
        auto: true,
    },
    Step {
        what: "the guest accepted",
        who: Who::Guests,
        all: &["coop-auto: accepting "],
        auto: true,
    },
    Step {
        what: "the guest is in the host's co-op session",
        who: Who::Guests,
        all: &["coop-live: guest: joined "],
        auto: false,
    },
    Step {
        what: "ping between the two",
        who: Who::Everyone,
        all: &["coop-live: ping ", " ms"],
        auto: false,
    },
    Step {
        what: "Start Game: the co-op check passed for two consoles",
        who: Who::Host,
        all: &["coop-live: lobby start state 7 ", " answered "],
        auto: false,
    },
    Step {
        what: "the session start counted the peers",
        who: Who::Host,
        all: &["session start: campaign with "],
        auto: false,
    },
    Step {
        what: "the session started",
        who: Who::Host,
        all: &["session start returned 1"],
        auto: false,
    },
    Step {
        what: "the host simulates (sync-server)",
        who: Who::Host,
        all: &["simulation mode sync-server"],
        auto: false,
    },
    Step {
        what: "the guest follows (sync-client)",
        who: Who::Guests,
        all: &["simulation mode sync-client"],
        auto: false,
    },
];

/// Lines that mean something went wrong, whatever the step.
const TROUBLE: [&str; 9] = [
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
            Who::Everyone => std::iter::once(roles.host.as_str())
                .chain(roles.guests.iter().map(String::as_str))
                .chain(roles.late.iter().map(String::as_str))
                .collect(),
        };
        let missing: Vec<String> = games
            .into_iter()
            .filter(|g| {
                !lines.iter().any(|(tag, text)| {
                    tag == g
                        && step.all.iter().all(|p| text.contains(p))
                        && !text.contains("ping: no answer")
                })
            })
            .map(String::from)
            .collect();
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
    Some(Verdict {
        steps,
        joins,
        trouble,
    })
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
/// of our rows drawn (`custom games: ...`, crates/boot/src/layout_glue.rs).
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
                .contains("[-]  7. the host invited the guest: no line without --auto")
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
                "coop-live: lobby start state 7 (co-op wants one console) answered 9 for two consoles",
            ),
            (
                "Alpha",
                "coop-probe: session start: campaign with 2 peers: showing the start 1 peer until OPTIONS_NEW",
            ),
            ("Alpha", "coop-probe: session start returned 1"),
            (
                "Alpha",
                "coop-probe: game start: options mode 1, simulation mode sync-server (2), x",
            ),
            (
                "Bravo",
                "coop-probe: game start: options mode 1, simulation mode sync-client (3), x",
            ),
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
}
