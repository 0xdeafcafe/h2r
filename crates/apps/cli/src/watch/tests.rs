//! `h2 play --watch`: the classifier, the debounce and the supervisor, over
//! fakes (no watcher, cargo, game or socket).

use super::*;

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn the_classifier_table() {
    use Kind::*;
    let table: &[(&str, Kind)] = &[
        // Build outputs, other worktrees, git's objects, editor droppings.
        ("target/release/h2", Ignored),
        ("crates/apps/cli/target/debug/x", Ignored),
        ("web/node_modules/vite/index.js", Ignored),
        ("web/dist/index.html", Ignored),
        (
            ".claude/worktrees/lane/crates/apps/cli/src/main.rs",
            Ignored,
        ),
        (".git/objects/ab/cdef", Ignored),
        (".git/logs/HEAD", Ignored),
        (".git/worktrees/lane/HEAD", Ignored),
        ("buck-out/v2/x", Ignored),
        ("crates/apps/cli/src/.main.rs.swp", Ignored),
        ("crates/apps/cli/src/main.rs~", Ignored),
        ("docs/.DS_Store", Ignored),
        ("", Ignored),
        // Git's state settles, builds nothing.
        (".git/HEAD", Git),
        (".git/refs/heads/master", Git),
        (".git/index.lock", Git),
        (".git/MERGE_HEAD", Git),
        (".git", Git),
        // Docs, tests and tooling.
        ("docs/runtime/app.md", Nothing),
        ("crates/apps/cli/README.md", Nothing),
        ("crates/apps/cli/tests/guard.rs", Nothing),
        ("crates/recompiler/emit/tests/split.rs", Nothing),
        ("crates/apps/cli/src/watch/tests.rs", Nothing),
        ("crates/extensions/game/src/hud_kit_tests.rs", Nothing),
        ("crates/apps/cli/benches/x.rs", Nothing),
        ("web/test/unit.test.ts", Nothing),
        ("scripts/docs-check.py", Nothing),
        ("deploy/host/image-tag", Nothing),
        ("justfile", Nothing),
        ("BUCK", Nothing),
        // Rules scripts.
        (
            "content/core/rules/slayer.rules",
            BuiltinRules,
        ),
        ("content/modes/rules/matchmaking/team_slayer.rules", BuiltinRules),
        ("scratch/my_mode.rules", Rules),
        // The web app.
        ("web/src/dash/page.tsx", Web),
        ("web/package.json", Web),
        ("web/bun.lock", Web),
        // The generator.
        ("crates/recompiler/lift/src/lib.rs", Image),
        ("crates/recompiler/emit/c/h2rt.h", Image),
        ("crates/recompiler/clif/src/lib.rs", Image),
        ("crates/formats/xbe/src/lib.rs", Image),
        ("crates/runtime/runtime/include/h2abi.h", Image),
        ("crates/runtime/recomp/c/boot_bridge.c", Image),
        ("crates/runtime/recomp/build.rs", Image),
        ("crates/runtime/recomp/src/render_entries.rs", Image),
        // Host Rust and baked data.
        ("crates/runtime/recomp/src/lib.rs", Host),
        ("crates/apps/cli/src/main.rs", Host),
        ("crates/apps/cli/Cargo.toml", Host),
        ("crates/extensions/game/build.rs", Host),
        ("crates/rewrites/hud-kit-core/src/lib.rs", Host),
        ("content/ui/screens/settings.yaml", Host),
        ("content/ui/design.yaml", Host),
        ("Cargo.toml", Host),
        ("Cargo.lock", Host),
        ("rust-toolchain.toml", Host),
        (".cargo/config.toml", Host),
    ];
    for (path, want) in table {
        assert_eq!(classify(path), *want, "{path}");
    }
}

#[test]
fn a_plan_gathers_what_its_paths_ask_for() {
    let p = Plan::of([
        "docs/a.md",
        "content/core/rules/ctf.rules",
        "web/src/x.ts",
        "target/release/h2",
    ]);
    assert_eq!(
        p.rules,
        BTreeSet::from(["content/core/rules/ctf.rules".to_string()])
    );
    assert!(p.web && p.host && !p.image);
    assert_eq!(
        p.hot(),
        Plan {
            rules: p.rules.clone(),
            web: true,
            ..Plan::default()
        }
    );
    assert_eq!(
        p.build(),
        Plan {
            host: true,
            ..Plan::default()
        }
    );
    assert!(Plan::of(["docs/a.md", "crates/x/tests/t.rs"]).is_empty());
    let rescan = Plan::of_changes(&Changes {
        paths: BTreeSet::new(),
        rescan: true,
    });
    assert!(rescan.host && rescan.image);
}

#[test]
fn relative_paths_name_the_git_dir_dot_git() {
    let root = Path::new("/r/checkout");
    let main = Path::new("/r/checkout/.git");
    let linked = Path::new("/r/main/.git/worktrees/lane");
    assert_eq!(
        relative(Path::new("/r/checkout/crates/a.rs"), root, main).as_deref(),
        Some("crates/a.rs")
    );
    assert_eq!(
        relative(Path::new("/r/checkout/.git/HEAD"), root, main).as_deref(),
        Some(".git/HEAD")
    );
    assert_eq!(
        relative(
            Path::new("/r/main/.git/worktrees/lane/index.lock"),
            root,
            linked
        )
        .as_deref(),
        Some(".git/index.lock")
    );
    assert_eq!(
        relative(Path::new("/r/main/.git/worktrees/lane"), root, linked).as_deref(),
        Some(".git")
    );
    assert_eq!(relative(Path::new("/elsewhere/x"), root, main), None);
}

#[test]
fn a_burst_settles_once_after_the_quiet_period() {
    let t0 = Instant::now();
    let mut d = Debounce::new(Duration::from_millis(1500));
    assert_eq!(d.poll(t0, false), Settle::Idle);
    d.event(t0, "crates/a.rs");
    d.event(t0 + Duration::from_millis(500), "crates/b.rs");
    d.event(t0 + SECOND, "crates/a.rs");
    assert_eq!(
        d.poll(t0 + Duration::from_millis(1200), false),
        Settle::Wait(Duration::from_millis(1300))
    );
    let Settle::Ready(c) = d.poll(t0 + Duration::from_millis(2500), false) else {
        panic!("not settled");
    };
    assert_eq!(
        c.paths,
        BTreeSet::from(["crates/a.rs".to_string(), "crates/b.rs".to_string()])
    );
    assert!(!c.rescan);
    // Taken once.
    assert_eq!(d.poll(t0 + 10 * SECOND, false), Settle::Idle);
}

#[test]
fn a_git_operation_in_progress_holds_the_change_set() {
    let t0 = Instant::now();
    let mut d = Debounce::new(Duration::from_millis(1500));
    d.event(t0, ".git/index.lock");
    d.event(t0, "crates/a.rs");
    // A long pull: quiet on disk, but the lock is still there.
    assert_eq!(d.poll(t0 + 5 * SECOND, true), Settle::Busy);
    assert_eq!(d.poll(t0 + 9 * SECOND, true), Settle::Busy);
    // Done: the quiet period starts again from the last busy look.
    assert_eq!(
        d.poll(t0 + 9 * SECOND + Duration::from_millis(500), false),
        Settle::Wait(SECOND)
    );
    let Settle::Ready(c) = d.poll(t0 + 11 * SECOND, false) else {
        panic!("not settled");
    };
    assert_eq!(c.paths, BTreeSet::from(["crates/a.rs".to_string()]));
}

#[test]
fn quiet_after_git_or_ignored_events_alone_is_nothing() {
    let t0 = Instant::now();
    let mut d = Debounce::new(Duration::from_millis(1500));
    d.event(t0, "target/release/h2");
    assert_eq!(
        d.poll(t0, false),
        Settle::Idle,
        "ignored paths start no timer"
    );
    d.event(t0, ".git/refs/remotes/origin/master");
    assert!(matches!(d.poll(t0, false), Settle::Wait(_)));
    assert_eq!(d.poll(t0 + 2 * SECOND, false), Settle::Idle);
    d.rescan(t0 + 3 * SECOND);
    let Settle::Ready(c) = d.poll(t0 + 5 * SECOND, false) else {
        panic!("a rescan settles");
    };
    assert!(c.rescan && c.paths.is_empty());
}

#[test]
fn git_operations_in_progress() {
    assert!(!git_busy_with(|_| false));
    for marker in [
        "index.lock",
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
    ] {
        assert!(git_busy_with(|n| n == marker), "{marker}");
    }
    assert!(!git_busy_with(|n| n == "ORIG_HEAD" || n == "FETCH_HEAD"));
}

// ---- The supervisor. ----

fn build(id: &str) -> Build {
    Build {
        commit: Commit {
            hash: id.into(),
            subject: format!("local changes ({id})"),
        },
        exe: PathBuf::from(format!("/c/builds/{id}/h2")),
        out: PathBuf::from(format!("/c/gen/{id}")),
        subjects: Vec::new(),
    }
}

fn status_of(actions: &[Action]) -> Option<Option<Notice>> {
    actions.iter().rev().find_map(|a| match a {
        Action::Status(n) => Some(n.clone()),
        _ => None,
    })
}

fn info(text: &str) -> Notice {
    Notice {
        kind: NoticeKind::Info,
        text: text.into(),
    }
}

fn host() -> Plan {
    Plan::of(["crates/apps/cli/src/main.rs"])
}

#[test]
fn docs_and_tests_do_nothing() {
    let mut s = Supervisor::new(build("local-0"));
    assert_eq!(
        s.start(),
        vec![Action::Status(None), Action::Launch(build("local-0"))]
    );
    assert!(
        s.handle(Event::Changes(Plan::of([
            "docs/a.md",
            "crates/a/tests/t.rs"
        ])))
        .is_empty()
    );
}

#[test]
fn a_rules_script_is_hot_with_no_restart() {
    let mut s = Supervisor::new(build("local-0"));
    let plan = Plan::of(["scratch/mode.rules"]);
    assert_eq!(
        s.handle(Event::Changes(plan.clone())),
        vec![Action::Hot(plan), Action::Status(None)]
    );
    let n = info("Reloaded Mode (in place)");
    assert_eq!(
        s.handle(Event::Hot(Some(n.clone()))),
        vec![Action::Status(Some(n))]
    );
    // Nothing reloaded: the banner stays.
    assert!(s.handle(Event::Hot(None)).is_empty());
}

#[test]
fn a_built_in_script_reloads_and_rebuilds_h2() {
    let mut s = Supervisor::new(build("local-0"));
    let a = s.handle(Event::Changes(Plan::of([
        "content/core/rules/slayer.rules",
    ])));
    assert!(matches!(&a[0], Action::Hot(p) if p.rules.len() == 1 && !p.builds()));
    assert!(
        matches!(&a[1], Action::StartBuild { id: 1, plan, .. } if plan.host && plan.rules.is_empty())
    );
}

#[test]
fn host_change_builds_offers_and_restarts() {
    let mut s = Supervisor::new(build("local-0"));
    let a = s.handle(Event::Changes(host()));
    assert_eq!(
        a,
        vec![
            Action::StartBuild {
                id: 1,
                plan: Plan {
                    host: true,
                    ..Plan::default()
                },
                keep: vec![
                    PathBuf::from("/c/builds/local-0"),
                    PathBuf::from("/c/gen/local-0")
                ],
            },
            Action::Status(Some(info("Rebuilding h2\u{2026}"))),
        ]
    );
    let a = s.handle(Event::Built(Ok(Some(build("local-1")))));
    let n = status_of(&a).unwrap().unwrap();
    assert_eq!(n.kind, NoticeKind::Ready);
    assert_eq!(
        n.text,
        "Update ready: local changes (local-1) \u{2014} press F5 to restart"
    );
    assert_eq!(
        s.handle(Event::ChildExited(RESTART_EXIT)),
        vec![Action::Status(None), Action::Launch(build("local-1"))]
    );
}

#[test]
fn an_image_change_says_so() {
    let mut s = Supervisor::new(build("local-0"));
    let a = s.handle(Event::Changes(Plan::of([
        "crates/recompiler/lift/src/lib.rs",
    ])));
    assert_eq!(
        status_of(&a),
        Some(Some(info("Rebuilding h2 and the game code\u{2026}")))
    );
}

#[test]
fn a_failed_build_keeps_the_game_and_is_carried_into_the_next() {
    let mut s = Supervisor::new(build("local-0"));
    s.handle(Event::Changes(Plan::of([
        "crates/recompiler/lift/src/lib.rs",
    ])));
    let a = s.handle(Event::Built(
        Err("error[E0308]: mismatched types".into()),
    ));
    assert_eq!(
        a,
        vec![Action::Status(Some(Notice {
            kind: NoticeKind::Error,
            text: "Local build failed: error[E0308]: mismatched types".into(),
        }))]
    );
    // A hot result does not hide the failure.
    let a = s.handle(Event::Hot(Some(info("Reloaded Slayer (in place)"))));
    assert_eq!(status_of(&a).unwrap().unwrap().kind, NoticeKind::Error);
    // The fix is a host-only change; the failed image work comes along.
    let a = s.handle(Event::Changes(host()));
    assert!(matches!(&a[0], Action::StartBuild { id: 2, plan, .. } if plan.host && plan.image));
}

#[test]
fn changes_during_a_build_merge_into_one_queued_build() {
    let mut s = Supervisor::new(build("local-0"));
    s.handle(Event::Changes(host()));
    let a = s.handle(Event::Changes(Plan::of([
        "crates/recompiler/emit/src/c.rs",
    ])));
    assert_eq!(a, vec![Action::Status(Some(info("Rebuilding h2\u{2026}")))]);
    s.handle(Event::Changes(Plan::of(["content/ui/design.yaml"])));
    let a = s.handle(Event::Built(Ok(Some(build("local-1")))));
    assert!(matches!(&a[0], Action::StartBuild { id: 2, plan, .. } if plan.host && plan.image));
    let n = status_of(&a).unwrap().unwrap();
    assert_eq!(n.kind, NoticeKind::Ready);
    assert!(n.text.ends_with("(rebuilding\u{2026})"), "{}", n.text);
}

#[test]
fn an_unchanged_build_offers_nothing() {
    let mut s = Supervisor::new(build("local-0"));
    s.handle(Event::Changes(host()));
    assert_eq!(
        s.handle(Event::Built(Ok(None))),
        vec![Action::Status(None)]
    );
}

#[test]
fn close_and_interrupt_end_it() {
    let mut s = Supervisor::new(build("local-0"));
    assert_eq!(
        s.handle(Event::ChildExited(0)),
        vec![Action::CancelBuild, Action::Exit(0)]
    );
    let mut s = Supervisor::new(build("local-0"));
    assert_eq!(
        s.handle(Event::Interrupt),
        vec![Action::CancelBuild, Action::TerminateChild]
    );
    assert!(s.handle(Event::Changes(host())).is_empty());
    assert_eq!(
        s.handle(Event::Interrupt),
        vec![Action::CancelBuild, Action::Exit(130)]
    );
}

#[test]
fn hot_notices() {
    assert_eq!(hot_notice(&[], &[]), None);
    assert_eq!(
        hot_notice(&["Reloaded Slayer (in place)".into()], &[]),
        Some(info("Reloaded Slayer (in place)"))
    );
    let n = hot_notice(
        &["Reloaded Slayer (in place)".into()],
        &["a.rules:3:1: expected `}`".into()],
    )
    .unwrap();
    assert_eq!(n.kind, NoticeKind::Error);
    assert_eq!(n.text, "Reload failed: a.rules:3:1: expected `}`");
}

#[test]
fn args_split_between_watcher_and_child() {
    let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    let w = parse_args(args(
        "--watch disc.iso --repo /r --quiet 800 --out /o --fresh-data --live-local",
    ))
    .unwrap();
    assert_eq!(w.repo, Some(PathBuf::from("/r")));
    assert_eq!(w.quiet, Duration::from_millis(800));
    assert!(w.fresh_data);
    assert_eq!(w.play, args("disc.iso --out /o --live-local"));
    assert_eq!(
        parse_args(args("--watch --quiet 5")).unwrap().quiet,
        Duration::from_millis(200)
    );
    assert!(parse_args(args("--watch --follow disc.iso")).is_err());
    assert!(parse_args(args("--watch --follow-status x")).is_err());
}

// ---- drive, over fakes. ----

/// Answers each build at once from a table, by id.
struct FakeBuilder {
    tx: Sender<Event>,
    results: Vec<Result<Option<Build>, String>>,
    started: Vec<(u64, Plan)>,
}
impl Builder for FakeBuilder {
    fn start(&mut self, id: u64, plan: &Plan, _: &[PathBuf]) {
        self.started.push((id, plan.clone()));
        let r = self.results.remove(0);
        self.tx.send(Event::Built(r)).unwrap();
    }
    fn cancel(&mut self) {}
}

/// Reloads at once, and records the game's pid it was given.
struct FakeHot {
    tx: Sender<Event>,
    ran: Vec<(Plan, u32)>,
}
impl HotRunner for FakeHot {
    fn run(&mut self, plan: &Plan, pid: u32) {
        self.ran.push((plan.clone(), pid));
        self.tx
            .send(Event::Hot(Some(info("Reloaded Mode (in place)"))))
            .unwrap();
    }
}

/// Each launch sends a scripted list of events.
struct FakeChild {
    tx: Sender<Event>,
    scripts: Vec<Vec<Event>>,
    launched: Vec<String>,
}
impl Launcher for FakeChild {
    fn launch(&mut self, b: &Build) -> Result<()> {
        self.launched.push(b.commit.hash.clone());
        for e in self.scripts.remove(0) {
            self.tx.send(e).unwrap();
        }
        Ok(())
    }
    fn terminate(&mut self) {}
    fn pid(&self) -> u32 {
        4242
    }
}

/// Presses F5 on a ready banner with nothing else pending; saves a fix
/// when a build fails.
struct FakeUser {
    tx: Sender<Event>,
    fixed: bool,
    seen: Vec<String>,
}
impl StatusSink for FakeUser {
    fn set(&mut self, n: Option<&Notice>) {
        let text = follow::format_status(n);
        match follow::parse_status(&text) {
            // The subject has parentheses of its own; a build in progress
            // or a failure is a suffix after the dash.
            Some(n)
                if n.kind == NoticeKind::Ready
                    && !n.text.rsplit('\u{2014}').next().unwrap_or("").contains('(') =>
            {
                self.tx.send(Event::ChildExited(RESTART_EXIT)).unwrap();
            }
            Some(n) if n.kind == NoticeKind::Error && !self.fixed => {
                self.fixed = true;
                self.tx.send(Event::Changes(host())).unwrap();
            }
            _ => {}
        }
        self.seen.push(text);
    }
}

#[test]
fn drive_reloads_builds_offers_restarts_and_ends_with_the_window() {
    let (tx, rx) = channel();
    let mut builder = FakeBuilder {
        tx: tx.clone(),
        results: vec![
            Err("error[E0308]: mismatched types".into()),
            Ok(Some(build("local-2"))),
        ],
        started: vec![],
    };
    let mut hot = FakeHot {
        tx: tx.clone(),
        ran: vec![],
    };
    let mut child = FakeChild {
        tx: tx.clone(),
        scripts: vec![
            vec![
                Event::Changes(Plan::of(["docs/a.md"])),
                Event::Changes(Plan::of(["scratch/mode.rules"])),
                Event::Changes(host()),
            ],
            vec![Event::ChildExited(0)],
        ],
        launched: vec![],
    };
    let mut user = FakeUser {
        tx,
        fixed: false,
        seen: vec![],
    };
    let code = drive(
        Supervisor::new(build("local-0")),
        &rx,
        &mut builder,
        &mut hot,
        &mut child,
        &mut user,
    );
    assert_eq!(code, 0);
    assert_eq!(hot.ran.len(), 1);
    assert_eq!(hot.ran[0].1, 4242, "the hot runner gets the game's pid");
    assert_eq!(
        builder
            .started
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(child.launched, vec!["local-0", "local-2"]);
    assert!(
        user.seen
            .iter()
            .any(|s| s == "error\tLocal build failed: error[E0308]: mismatched types\n")
    );
    assert!(
        user.seen
            .iter()
            .any(|s| s.starts_with("ready\tUpdate ready"))
    );
    // Cleared at each launch, and after the rules change (its reload's
    // banner arrives while the build shows).
    assert_eq!(user.seen.iter().filter(|s| s.is_empty()).count(), 3);
}
