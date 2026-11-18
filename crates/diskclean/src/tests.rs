use super::*;

struct Tmp(PathBuf);
impl Tmp {
    fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!("diskclean-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        // Canonical, so paths match what git reports (/var -> /private/var).
        Tmp(p.canonicalize().unwrap())
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_mtime(p: &Path, t: SystemTime) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as libc::time_t;
    let tv = [libc::timeval { tv_sec: secs, tv_usec: 0 }; 2];
    let c = CString::new(p.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::utimes(c.as_ptr(), tv.as_ptr()) }, 0);
}

fn facts(entry: WorktreeEntry) -> WorktreeFacts {
    WorktreeFacts { entry, merged: true, clean: true, last_activity: SystemTime::UNIX_EPOCH, in_use: false }
}

fn linked(branch: Option<&str>) -> WorktreeEntry {
    WorktreeEntry { path: "/w".into(), branch: branch.map(Into::into), ..Default::default() }
}

#[test]
fn parses_porcelain_with_main_first_and_flags() {
    let s = "worktree /repo\nHEAD aa\nbranch refs/heads/master\n\n\
             worktree /repo/.wt/a\nHEAD bb\nbranch refs/heads/a\nlocked claude agent\n\n\
             worktree /tmp/x\nHEAD cc\ndetached\nprunable gitdir file points to non-existent location\n";
    let w = parse_worktrees(s);
    assert_eq!(w.len(), 3);
    assert!(w[0].main && w[0].branch.as_deref() == Some("master"));
    assert!(!w[1].main && w[1].locked && w[1].branch.as_deref() == Some("a"));
    assert!(w[2].prunable && w[2].branch.is_none() && !w[2].locked);
}

#[test]
fn only_a_merged_clean_idle_unused_branch_worktree_goes() {
    let now = SystemTime::now();
    let ok = facts(linked(Some("lane")));
    assert_eq!(decide_worktree(&ok, now, DAY), Ok("lane".to_string()));

    let cases: Vec<(WorktreeFacts, &str)> = vec![
        (facts(WorktreeEntry { main: true, ..linked(Some("master")) }), "main"),
        (facts(WorktreeEntry { locked: true, ..linked(Some("lane")) }), "locked"),
        (facts(WorktreeEntry { prunable: true, ..linked(Some("lane")) }), "prunable"),
        (facts(linked(None)), "detached"),
        (WorktreeFacts { merged: false, ..ok.clone() }, "not merged"),
        (WorktreeFacts { clean: false, ..ok.clone() }, "changes"),
        (WorktreeFacts { last_activity: now - DAY / 2, ..ok.clone() }, "recently"),
        (WorktreeFacts { in_use: true, ..ok.clone() }, "in use"),
    ];
    for (f, why) in cases {
        let r = decide_worktree(&f, now, DAY);
        assert!(r.as_ref().is_err_and(|e| e.contains(why)), "{why}: {r:?}");
    }
}

fn fake_target(dir: &Path) {
    fs::create_dir_all(dir.join("debug/deps")).unwrap();
    fs::create_dir_all(dir.join("debug/.fingerprint")).unwrap();
    fs::create_dir_all(dir.join("debug/incremental")).unwrap();
    fs::write(dir.join("CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55\n").unwrap();
    fs::write(dir.join(".rustc_info.json"), "{}").unwrap();
}

#[test]
fn finds_targets_by_cargo_markers_and_not_other_folders() {
    let t = Tmp::new("find");
    fake_target(&t.0.join("a/release-target"));
    fake_target(&t.0.join("b/c/r2-target"));
    fs::create_dir_all(t.0.join("run/h2boot/deps")).unwrap(); // a run folder: never a target
    fs::write(t.0.join("run/CACHEDIR.TAG"), "").unwrap(); // a tag alone isn't Cargo's
    fake_target(&t.0.join("a/release-target/nested-target")); // never looked inside a target
    let mut out = BTreeSet::new();
    find_targets(&t.0, 3, &mut out);
    let names: Vec<_> = out.iter().map(|p| p.strip_prefix(&t.0).unwrap().to_path_buf()).collect();
    assert_eq!(names, [PathBuf::from("a/release-target"), PathBuf::from("b/c/r2-target")]);
}

#[test]
fn old_artifacts_are_the_ones_no_build_wrote_lately() {
    let t = Tmp::new("old");
    let tg = t.0.join("target");
    fake_target(&tg);
    let now = SystemTime::now();
    for (name, days) in [("libold-1.rlib", 3), ("libnew-2.rlib", 0)] {
        let p = tg.join("debug/deps").join(name);
        fs::write(&p, "x").unwrap();
        set_mtime(&p, now - DAY * days);
    }
    let inc = tg.join("debug/incremental/old-3");
    fs::create_dir_all(&inc).unwrap();
    set_mtime(&inc, now - DAY * 5);
    // A triple-specific profile counts too.
    fs::create_dir_all(tg.join("wasm32-unknown-unknown/release/.fingerprint")).unwrap();
    fs::create_dir_all(tg.join("wasm32-unknown-unknown/release/deps")).unwrap();
    let w = tg.join("wasm32-unknown-unknown/release/deps/libw-4.rlib");
    fs::write(&w, "x").unwrap();
    set_mtime(&w, now - DAY * 4);

    let old = old_artifacts(&tg, now, 2 * DAY);
    let rel: Vec<_> = old.iter().map(|p| p.strip_prefix(&tg).unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(rel, ["debug/deps/libold-1.rlib", "debug/incremental/old-3", "wasm32-unknown-unknown/release/deps/libw-4.rlib"]);
}

#[test]
fn target_activity_follows_the_newest_build() {
    let t = Tmp::new("act");
    let tg = t.0.join("target");
    fake_target(&tg);
    let now = SystemTime::now();
    let old = now - DAY * 10;
    for p in [tg.clone(), tg.join(".rustc_info.json"), tg.join("debug"), tg.join("debug/deps"), tg.join("debug/.fingerprint")] {
        set_mtime(&p, old);
    }
    assert!(age(now, target_activity(&tg)) >= 9 * DAY);
    fs::write(tg.join("debug/.cargo-lock"), "").unwrap();
    assert!(age(now, target_activity(&tg)) < DAY);
}

#[test]
fn freeable_skips_hard_linked_files() {
    let t = Tmp::new("free");
    fs::write(t.0.join("own"), vec![1u8; 64 * 1024]).unwrap();
    fs::write(t.0.join("shared"), vec![1u8; 64 * 1024]).unwrap();
    fs::hard_link(t.0.join("shared"), t.0.join("store-copy")).unwrap();
    let own = freeable(&t.0.join("own"));
    assert!(own >= 64 * 1024);
    assert_eq!(freeable(&t.0.join("shared")), 0);
}

fn sh(dir: &Path, args: &[&str]) {
    let o = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

/// A repository with four worktrees: merged and clean, merged with an
/// untracked file, unmerged, and locked. Only the first goes, its branch
/// with it; a stale target in a scanned folder goes, a fresh one stays.
#[test]
fn plans_and_applies_on_a_real_repository() {
    let t = Tmp::new("repo");
    let repo = t.0.join("repo");
    fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "master"]);
    sh(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"]);
    let wt = t.0.join("wt");
    for name in ["merged", "dirty", "ahead", "held"] {
        sh(&repo, &["worktree", "add", "-q", "-b", name, &wt.join(name).to_string_lossy()]);
    }
    fs::write(wt.join("dirty/new.txt"), "work").unwrap();
    sh(&wt.join("ahead"), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "b"]);
    sh(&repo, &["worktree", "lock", &wt.join("held").to_string_lossy()]);

    let scan = t.0.join("scratch");
    fake_target(&scan.join("s1/old-target"));
    fake_target(&scan.join("s2/new-target"));
    let stale = SystemTime::now() - DAY * 5;
    let old = scan.join("s1/old-target");
    for p in [old.clone(), old.join(".rustc_info.json"), old.join("debug"), old.join("debug/deps"), old.join("debug/.fingerprint")] {
        set_mtime(&p, stale);
    }
    fs::create_dir_all(scan.join("run/h2boot")).unwrap();

    // Nothing is a day old yet: only the stale target goes.
    let mut o = Options::new(repo.clone());
    o.scan = vec![scan.clone()];
    let p = plan(&o).unwrap();
    assert_eq!(p.actions, [Action::RemoveTarget { path: old.clone() }], "kept: {:?}", p.kept);

    // A day and a half later the merged worktree is idle too; the fresh target
    // isn't yet.
    o.now = SystemTime::now() + DAY * 3 / 2;
    let p = plan(&o).unwrap();
    let wts: Vec<_> = p.actions.iter().filter(|a| matches!(a, Action::RemoveWorktree { .. })).collect();
    assert_eq!(wts, [&Action::RemoveWorktree { path: wt.join("merged"), branch: "merged".into() }], "kept: {:?}", p.kept);
    for a in &p.actions {
        apply(&o, a).unwrap();
    }
    assert!(!wt.join("merged").exists());
    assert!(wt.join("dirty/new.txt").exists() && wt.join("ahead").exists() && wt.join("held").exists());
    assert!(!old.exists() && scan.join("s2/new-target").exists() && scan.join("run/h2boot").exists());
    let branches = Command::new("git").arg("-C").arg(&repo).args(["branch", "--list", "merged"]).output().unwrap();
    assert!(branches.stdout.is_empty(), "branch merged should be deleted");
}

#[test]
fn a_target_a_process_names_is_in_use() {
    let t = Tmp::new("use");
    let tg = t.0.join("busy-target");
    fs::create_dir_all(&tg).unwrap();
    assert!(!in_use(&tg));
    // Named in its arguments. (Its environment counts too, but macOS hides
    // the environment of platform binaries like sleep from ps; Homebrew's
    // cargo and rustc show theirs.)
    let mut child = Command::new("sh").arg("-c").arg("sleep 30; :").arg(&tg).spawn().unwrap();
    let busy = in_use(&tg);
    child.kill().unwrap();
    let _ = child.wait();
    assert!(busy);
}
