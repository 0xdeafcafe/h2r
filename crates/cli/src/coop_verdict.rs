//! The co-op test's log and checklist (`h2 coop test`, `h2 coop verdict`;
//! docs/live/live-coop.md section 12): which lines of each game go into the
//! merged log, how a merged line reads, and which checklist steps a log
//! shows.
//!
//! A merged log starts with one `# host <gamertag>` line and a
//! `# guest <gamertag>` line per guest, then one line per game line:
//! `[+SECONDS] GAMERTAG | TEXT`.

use std::fmt::Write as _;

/// Whether a game's stderr line goes into the merged log.
pub fn wanted(line: &str) -> bool {
    const KEEP: [&str; 9] = [
        "coop-live",
        "coop-probe",
        "coop-auto",
        "skulls",
        "session",
        "game start",
        "live: playing as",
        "custom games",
        "panicked",
    ];
    KEEP.iter().any(|k| line.contains(k))
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
}

impl Roles {
    pub fn header(&self) -> String {
        let mut s = format!("# host {}\n", self.host);
        for g in &self.guests {
            let _ = writeln!(s, "# guest {g}");
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
const TROUBLE: [&str; 8] = [
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub steps: Vec<StepResult>,
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
        steps.push(StepResult {
            n: i + 1,
            what: step.what,
            seen,
        });
    }
    let trouble = log
        .lines()
        .filter(|l| parse_line(l).is_some_and(|(_, t)| TROUBLE.iter().any(|p| t.contains(p))))
        .map(String::from)
        .collect();
    Some(Verdict { steps, trouble })
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
        }
        match self.stopped_at() {
            Some(step) => {
                let _ = writeln!(s, "stopped at step {}: {}", step.n, step.what);
            }
            None => s.push_str("every step seen\n"),
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
                "coop-probe: game start: options mode 1, simulation mode sync-server (3), x",
            ),
            (
                "Bravo",
                "coop-probe: game start: options mode 1, simulation mode sync-client (2), x",
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
}
