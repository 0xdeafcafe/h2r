//! The lab's one detached checkout, its one Cargo target, the game images
//! (one work directory per build variant) and the bounded build caches.
//! See docs/runtime/lab.md, "Builds go by branch".

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};

use super::{host::HostConfig, proto, transfer};

pub(super) const DEFAULT_OBJECT_LIMIT: u64 = 8 << 30;
pub(super) const DEFAULT_TARGET_LIMIT: u64 = 32 << 30;

/// The image work directories kept, the one in use included. Each holds
/// the generated C, the object pins, the module and `h2` of its variant.
pub(super) const IMAGE_VARIANTS: usize = 4;

pub(super) fn workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2).max(1))
}

// A lab-root child being cleaned or reused must be an actual directory.
pub(super) fn ensure_dir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(path)?.is_dir(),
                "{} isn't a directory",
                path.display()
            );
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("creating {}", path.display())),
    }
}

/// The one Cargo target every lab build uses. Image jobs and host updates
/// both build `src/current` into it, so one source tree has one target
/// and a commit both of them build is compiled once.
pub(super) fn target_dir(cfg: &HostConfig) -> PathBuf {
    cfg.dir("target")
}

/// `H2_OBJ_CACHE` for every image build: `obj` beside the lab root. With
/// the default root (`~/Library/Caches/h2recomp/lab`) that is
/// `~/Library/Caches/h2recomp/obj`, the object cache the Mac's own
/// `h2 build`s use (AGENTS.md, "Build fast"), so lab jobs, other
/// checkouts and worktrees share clang's shard objects.
pub(super) fn obj_cache_dir(cfg: &HostConfig) -> PathBuf {
    cfg.root
        .parent()
        .map_or_else(|| cfg.dir("obj-cache"), |parent| parent.join("obj"))
}

/// The `h2` a lab cargo build produces.
pub(super) fn built_h2(cfg: &HostConfig) -> PathBuf {
    target_dir(cfg).join("release/h2")
}

/// `cargo build --release -p cli --bin h2` of `src` into the lab's target,
/// for jobs and host updates alike. The host's `H2_*` variables are
/// dropped and a job's build variables are not passed: they pick the game
/// image, not the Rust build, and leaving them out keeps Cargo's
/// fingerprints (and a rustc wrapper's keys) the same for every variant.
/// Cargo's own configuration applies as in a Terminal, including a
/// `build.rustc-wrapper` such as kache; the lab never sets one.
pub(super) fn cargo_command(cfg: &HostConfig, src: &Path, features: &[String]) -> Command {
    let mut cargo = crate::follow::niced(&cfg.cargo);
    cargo.args(["build", "--release", "-p", "cli", "--bin", "h2"]);
    if !features.is_empty() {
        cargo.arg("--features").arg(features.join(","));
    }
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("H2_")) {
            cargo.env_remove(k);
        }
    }
    cargo
        .current_dir(src)
        .env("CARGO_BUILD_JOBS", workers().to_string())
        .env("CARGO_TARGET_DIR", target_dir(cfg));
    cargo
}

/// The rustc wrapper Cargo will use for a build in `src`, and where it is
/// set: `RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WRAPPER`, then
/// `build.rustc-wrapper` in the nearest `.cargo/config.toml` (or
/// `.cargo/config`) above `src`, then in `$CARGO_HOME` (default
/// `~/.cargo`). An empty value turns the wrapper off. This is for the
/// build log only: Cargo applies the wrapper itself.
pub(super) fn rustc_wrapper(
    src: &Path,
    var: impl Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    for name in ["RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER"] {
        if let Some(v) = var(name) {
            return (!v.is_empty()).then(|| (v, name.to_string()));
        }
    }
    let home = var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| var("HOME").map(|h| Path::new(&h).join(".cargo")));
    let dirs = src.ancestors().map(|a| a.join(".cargo")).chain(home);
    for dir in dirs {
        for file in ["config.toml", "config"] {
            let path = dir.join(file);
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(table) = text.parse::<toml::Table>() else {
                continue;
            };
            if let Some(v) = table
                .get("build")
                .and_then(|b| b.get("rustc-wrapper"))
                .and_then(|w| w.as_str())
            {
                return (!v.is_empty()).then(|| (v.to_string(), path.display().to_string()));
            }
        }
    }
    None
}

/// One line for the build log: how many crates Cargo compiled and the
/// rustc wrapper in effect. Cargo prints a `Compiling` line for every
/// crate it builds; the others were fresh in the target.
pub(super) fn cargo_summary(output: &str, wrapper: Option<&(String, String)>) -> String {
    let compiled = output
        .lines()
        .filter(|l| l.trim_start().starts_with("Compiling "))
        .count();
    let wrapper = match wrapper {
        Some((w, from)) => format!("rustc wrapper {w} (from {from})"),
        None => "no rustc wrapper, so a new target compiles every crate".into(),
    };
    format!("lab: cargo compiled {compiled} crates, the rest were fresh in the lab target; {wrapper}")
}

/// One line for the build log from `h2 build`'s phase lines: whether the
/// analysis came from the boot cache, how many C files emission rewrote,
/// and how many objects clang compiled or took from the object cache.
pub(super) fn image_summary(output: &str) -> String {
    let mut parts = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("analyze: ") {
            parts.push(if rest.contains("(cached)") {
                "analysis cached".to_string()
            } else {
                "analysis recomputed".to_string()
            });
        } else if let Some(rest) = line.strip_prefix("emit: ") {
            let files = number_before(rest, " files (");
            let rewritten = number_before(rest, " rewritten)");
            if let (Some(files), Some(rewritten)) = (files, rewritten) {
                parts.push(format!("emit rewrote {rewritten}/{files} C files"));
            }
        } else if line.starts_with("clang (") {
            let compiled = number_before(line, " compiled");
            let reused = number_before(line, " reused");
            if let (Some(compiled), Some(reused)) = (compiled, reused) {
                parts.push(format!("clang reused {reused}/{} shards ({compiled} compiled)", compiled + reused));
            }
        }
    }
    if parts.is_empty() {
        "lab: h2 build printed no phase lines".into()
    } else {
        format!("lab: image reuse: {}", parts.join(", "))
    }
}

/// The decimal number just before the first `marker` in `s`.
fn number_before(s: &str, marker: &str) -> Option<u64> {
    let head = &s[..s.find(marker)?];
    let start = head
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    head[start..].parse().ok()
}

/// What a step wrote to `log` after `offset`, at most its last 4 MiB.
pub(super) fn log_since(log: &Path, offset: u64) -> String {
    const MAX: u64 = 4 << 20;
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let from = offset.max(len.saturating_sub(MAX));
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_ok() {
        let _ = file.take(MAX).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The build variant: everything that picks an image except the commit
/// (sorted features, build variables, the PGO profile's content hash).
/// Every commit of one variant builds in the same work directory, so
/// `h2 build` rewrites only the C that changed and clang compiles only
/// those shards; another `H2_OPT` or feature set gets its own directory.
pub(super) fn variant_key(
    features: &[String],
    env: &BTreeMap<String, String>,
    pgo_hash: &str,
) -> String {
    let mut s = String::from("variant\n");
    for f in features {
        s += &format!("feature {f}\n");
    }
    for (k, v) in env {
        s += &format!("env {k}={v}\n");
    }
    s += &format!("pgo {pgo_hash}\n");
    proto::to_hex(&Sha256::digest(s.as_bytes()))[..16].to_string()
}

/// `variants/<key>`: the variant's work directory. `h2 build` uses it as
/// its `--out`, and it is the published image its `image.json` names.
pub(super) fn variant_dir(cfg: &HostConfig, key: &str) -> PathBuf {
    cfg.dir("variants").join(key)
}

/// Holds a variant's work directory for one build. The activity lock
/// already allows one build at a time; this refuses a second builder of
/// the same directory outright rather than letting two interleave.
pub(super) fn lock_variant(dir: &Path) -> Result<File> {
    let path = dir.join("lab-build.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => bail!("another build is using {}", dir.display()),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

/// The commit a work directory's stamp names: what a rebuild starts from.
pub(super) fn stamped_commit(dir: &Path) -> Option<String> {
    let stamp = fs::read(dir.join("image.json")).ok()?;
    let value = serde_json::from_slice::<serde_json::Value>(&stamp).ok()?;
    value["commit"].as_str().map(str::to_owned)
}

/// Marks a variant used, so pruning keeps the ones used last.
pub(super) fn touch(dir: &Path) {
    let _ = File::options()
        .append(true)
        .open(dir.join("image.json"))
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// Keeps `keep` and the other work directories used last (by their stamp,
/// else the directory), [`IMAGE_VARIANTS`] in all. Only real directories
/// named like a variant key are removed; anything else is left alone.
pub(super) fn prune_variants(cfg: &HostConfig, keep: &Path, log: &mut impl Write) -> Result<()> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(cfg.dir("variants"))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let ours =
            name.len() == 16 && name.as_encoded_bytes().iter().all(u8::is_ascii_hexdigit);
        if !ours || !entry.file_type()?.is_dir() || path == keep {
            continue;
        }
        let used = fs::metadata(path.join("image.json"))
            .or_else(|_| entry.metadata())
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        dirs.push((used, path));
    }
    for path in variants_to_remove(dirs, IMAGE_VARIANTS.saturating_sub(1)) {
        writeln!(log, "lab: removing image work directory {}", path.display())?;
        fs::remove_dir_all(&path)?;
    }
    Ok(())
}

/// Those of `dirs` past the `keep` used last.
fn variants_to_remove(mut dirs: Vec<(SystemTime, PathBuf)>, keep: usize) -> Vec<PathBuf> {
    dirs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    dirs.into_iter().skip(keep).map(|(_, p)| p).collect()
}

pub(super) fn checkout_command(cfg: &HostConfig, commit: &str) -> Result<Command> {
    let parent = cfg.dir("src");
    ensure_dir(&parent)?;
    let checkout = parent.join("current");
    let exists =
        crate::follow::owned_worktree(&cfg.repo, &parent, "current").map_err(anyhow::Error::msg)?;
    let mut cmd = transfer::git(if exists { &checkout } else { &cfg.repo });
    if exists {
        // Restore tracked files while checking out the commit. Build outputs
        // live outside src/current. No untracked checkout-wide clean.
        cmd.args(["checkout", "--detach", "--force", commit]);
    } else {
        cmd.args(["worktree", "add", "--force", "--detach"])
            .arg(checkout)
            .arg(commit);
    }
    Ok(cmd)
}

pub(super) fn matches_stamp(dir: &Path, key: &str) -> Result<bool> {
    let stamp = match fs::read(dir.join("image.json")) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        other => other?,
    };
    let value = serde_json::from_slice::<serde_json::Value>(&stamp).unwrap_or_default();
    Ok(value["key"].as_str() == Some(key)
        && dir.join("h2").is_file()
        && dir
            .join(emit::compile::Arch::host().module_file_name())
            .is_file())
}

/// Old hosts hold the activity lock in the daemon. After an abnormal exit,
/// a child may still be alive with its stdout/stderr redirected to the
/// job logs. Check those files before migrating or admitting another job.
/// Do not pass an inheritable lock to build tools (their cache daemons can
/// outlive a build). Healthy restarts have no interrupted records to check.
pub(super) fn check_abandoned(run: &Path) -> Result<()> {
    let files: Vec<_> = ["run.log", "build.log"]
        .into_iter()
        .map(|log| run.join(log))
        .filter(|log| log.is_file())
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    let open = Command::new("lsof")
        .args(["-nP", "-w", "-t", "-a", "-d", "1,2", "--"])
        .args(files)
        .output()
        .context("checking interrupted lab job logs with lsof")?;
    ensure!(
        open.stdout.is_empty() && open.status.code() == Some(1),
        "cannot recover interrupted job {} while its logs may have a live writer (pids {}, {}); stop the old job before starting the lab host",
        run.display(),
        String::from_utf8_lossy(&open.stdout).trim(),
        String::from_utf8_lossy(&open.stderr).trim()
    );
    Ok(())
}

/// Old images have 16-hex keys and old linked checkouts have full git IDs.
/// Select only direct, real children of the lab-owned cache directories.
/// Git removes both the worktree and its registration in one operation.
pub(super) fn migrate(cfg: &HostConfig) -> Result<()> {
    let src = cfg.dir("src");
    ensure_dir(&src)?;
    for entry in fs::read_dir(&src)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if proto::check_commit(name).is_err() || !entry.file_type()?.is_dir() {
            continue;
        }
        if !crate::follow::owned_worktree(&cfg.repo, &src, name).map_err(anyhow::Error::msg)? {
            continue;
        }
        let status = transfer::git(&cfg.repo)
            .args(["worktree", "remove", "--force"])
            .arg(entry.path())
            .status()?;
        ensure!(
            status.success(),
            "removing old lab checkout {}",
            entry.path().display()
        );
    }
    // The lab's private object cache: objects live in the shared one beside
    // the root. Its objects become the shared cache when there is none yet,
    // so the first build after the move is still warm.
    let private = cfg.dir("obj-cache");
    let shared = obj_cache_dir(cfg);
    if shared != private && fs::symlink_metadata(&private).is_ok_and(|m| m.is_dir()) {
        if fs::symlink_metadata(&shared).is_err() {
            fs::rename(&private, &shared)?;
        } else {
            fs::remove_dir_all(&private)?;
        }
    }
    // The single image directory every variant once shared; images live in
    // variants/<key>.
    let image = cfg.dir("image");
    if fs::symlink_metadata(&image).is_ok_and(|m| m.is_dir()) {
        fs::remove_dir_all(&image)?;
    }
    let images = cfg.dir("images");
    match fs::symlink_metadata(&images) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Ok(meta) if !meta.is_dir() => return Ok(()),
        other => {
            other?;
        }
    }
    for entry in fs::read_dir(&images)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        if name.len() == 16 && name.iter().all(u8::is_ascii_hexdigit) && entry.file_type()?.is_dir()
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    if fs::read_dir(&images)?.next().is_none() {
        fs::remove_dir(images)?;
    }
    Ok(())
}

/// Cargo may not notice removal of individual build-script outputs. Keep
/// the whole target warm while it fits and clear it as a unit otherwise.
fn size(dir: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let len = if kind.is_dir() {
            size(&entry.path())?
        } else if kind.is_file() {
            entry.metadata()?.len()
        } else {
            0
        };
        bytes = bytes.saturating_add(len);
    }
    Ok(bytes)
}

pub(super) fn prune(cfg: &HostConfig, log: &mut impl Write) -> Result<()> {
    let objects = obj_cache_dir(cfg);
    ensure_dir(&objects)?;
    let pruned = emit::compile::prune_obj_cache(&objects, cfg.obj_cache_limit, &Default::default())
        .map_err(anyhow::Error::msg)?;
    if pruned.removed != 0 {
        writeln!(
            log,
            "lab: pruned {} cache objects, {} bytes remain",
            pruned.removed, pruned.kept
        )?;
    }
    // The cache is shared with the Mac's own builds, which may add objects
    // while this prunes; that is no reason to fail the job.
    if pruned.kept > cfg.obj_cache_limit {
        writeln!(
            log,
            "lab: object cache {} holds {} bytes, over its {} byte limit",
            objects.display(),
            pruned.kept,
            cfg.obj_cache_limit
        )?;
    }
    let target = cfg.dir("target");
    ensure_dir(&target)?;
    let bytes = size(&target)?;
    if bytes > cfg.target_limit {
        writeln!(
            log,
            "lab: clearing Cargo target ({bytes} bytes exceeds {})",
            cfg.target_limit
        )?;
        fs::remove_dir_all(&target)?;
        fs::create_dir(target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::host::image_key;
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("h2lab-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(root: &Path) -> HostConfig {
        HostConfig {
            root: root.to_owned(),
            socket: root.join("lab.sock"),
            repo: root.join("repo"),
            disc: root.join("halo2.iso"),
            h2: "/bin/true".into(),
            out: None,
            data: root.join("data"),
            cargo: "cargo".into(),
            obj_cache_limit: 0,
            target_limit: 0,
            max_jobs: 1,
            lan: None,
            managed: false,
            explicit_h2: true,
            grace: Duration::from_secs(1),
            takeover: Default::default(),
        }
    }

    fn env(v: &[(&str, &str)]) -> BTreeMap<String, String> {
        v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn one_variant_builds_every_commit_in_one_work_directory() {
        let cfg = config(Path::new("/lab"));
        let opt2_env = env(&[("H2_OPT", "2")]);
        let opt2 = variant_key(&[], &opt2_env, "");
        assert_eq!(opt2, variant_key(&[], &env(&[("H2_OPT", "2")]), ""));
        assert_eq!(variant_dir(&cfg, &opt2), Path::new("/lab/variants").join(&opt2));
        assert_eq!(opt2.len(), 16);
        assert!(opt2.bytes().all(|b| b.is_ascii_hexdigit()));
        // The commit picks the image within the directory, not the directory.
        assert_ne!(image_key("aaa", &[], &opt2_env, ""), image_key("bbb", &[], &opt2_env, ""));
        // Anything else that picks an image gets its own directory.
        for other in [
            variant_key(&[], &env(&[("H2_OPT", "3")]), ""),
            variant_key(&[], &BTreeMap::new(), ""),
            variant_key(&["perf".into()], &opt2_env, ""),
            variant_key(&[], &opt2_env, "f00d"),
        ] {
            assert_ne!(opt2, other);
            assert_ne!(variant_dir(&cfg, &opt2), variant_dir(&cfg, &other));
        }
    }

    #[test]
    fn the_work_directory_is_the_published_image_of_its_last_commit() {
        let root = scratch("publish");
        let cfg = config(&root);
        let dir = variant_dir(&cfg, &variant_key(&[], &BTreeMap::new(), ""));
        fs::create_dir_all(&dir).unwrap();
        let first = image_key("aaa", &[], &BTreeMap::new(), "");
        let second = image_key("bbb", &[], &BTreeMap::new(), "");
        assert!(!matches_stamp(&dir, &first).unwrap());
        assert_eq!(stamped_commit(&dir), None);
        fs::write(dir.join("h2"), b"h2").unwrap();
        fs::write(dir.join(emit::compile::Arch::host().module_file_name()), b"m").unwrap();
        let stamp = serde_json::json!({ "key": first, "commit": "aaa" });
        fs::write(dir.join("image.json"), stamp.to_string()).unwrap();
        assert!(matches_stamp(&dir, &first).unwrap());
        assert!(!matches_stamp(&dir, &second).unwrap());
        assert_eq!(stamped_commit(&dir).as_deref(), Some("aaa"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_second_builder_of_one_work_directory_is_refused() {
        let root = scratch("lock");
        let held = lock_variant(&root).unwrap();
        assert!(format!("{:#}", lock_variant(&root).unwrap_err()).contains("another build"));
        drop(held);
        lock_variant(&root).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pruning_keeps_the_variants_used_last_and_leaves_other_entries() {
        let root = scratch("prune");
        let cfg = config(&root);
        let variants = cfg.dir("variants");
        let now = SystemTime::now();
        let mut keys = Vec::new();
        for age in 0..6u64 {
            let key = format!("{age:016x}");
            let dir = variants.join(&key);
            fs::create_dir_all(&dir).unwrap();
            let stamp = File::create(dir.join("image.json")).unwrap();
            stamp.set_modified(now - Duration::from_secs(60 * (age + 1))).unwrap();
            keys.push(key);
        }
        fs::create_dir_all(variants.join("other")).unwrap();
        // The variant in use stays even when it is the oldest.
        let keep = variants.join(&keys[5]);
        let mut log = Vec::new();
        prune_variants(&cfg, &keep, &mut log).unwrap();
        let mut left: Vec<String> = fs::read_dir(&variants)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        let mut want = vec![
            keys[0].clone(),
            keys[1].clone(),
            keys[2].clone(),
            keys[5].clone(),
            "other".to_string(),
        ];
        want.sort();
        assert_eq!(left, want);
        assert_eq!(String::from_utf8(log).unwrap().lines().count(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn variants_past_the_limit_go_oldest_first() {
        let t = |s| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let dirs: Vec<(SystemTime, PathBuf)> =
            vec![(t(1), "a".into()), (t(3), "c".into()), (t(2), "b".into())];
        assert_eq!(variants_to_remove(dirs.clone(), 1), [PathBuf::from("b"), PathBuf::from("a")]);
        assert!(variants_to_remove(dirs, 3).is_empty());
    }

    #[test]
    fn jobs_and_updates_share_one_target_and_keep_the_users_wrapper() {
        let cfg = config(Path::new("/lab"));
        let cmd = cargo_command(&cfg, Path::new("/lab/src/current"), &["perf".into()]);
        let args: Vec<String> =
            cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let want = ["build", "--release", "-p", "cli", "--bin", "h2", "--features", "perf"];
        assert!(args.ends_with(&want.map(String::from)), "{args:?}");
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/lab/src/current")));
        let set: BTreeMap<String, std::ffi::OsString> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_owned())))
            .collect();
        assert_eq!(set["CARGO_TARGET_DIR"].as_os_str(), Path::new("/lab/target").as_os_str());
        assert_eq!(built_h2(&cfg), Path::new("/lab/target/release/h2"));
        // Image builds share the Mac's object cache beside the lab root.
        let default = config(Path::new("/Users/me/Library/Caches/h2recomp/lab"));
        assert_eq!(obj_cache_dir(&default), Path::new("/Users/me/Library/Caches/h2recomp/obj"));
        assert_eq!(obj_cache_dir(&config(Path::new("/"))), Path::new("/obj-cache"));
        for key in set.keys() {
            assert!(!key.starts_with("H2_"), "{key}");
            assert!(key != "RUSTC_WRAPPER" && key != "SCCACHE_DIR", "{key}");
        }
    }

    #[test]
    fn the_wrapper_comes_from_the_environment_then_the_nearest_config() {
        let root = scratch("wrapper");
        let src = root.join("lab/src/current");
        fs::create_dir_all(&src).unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join(".cargo")).unwrap();
        fs::write(home.join(".cargo/config.toml"), "[build]\nrustc-wrapper = \"/bin/kache\"\n")
            .unwrap();
        let home = home.to_string_lossy().into_owned();
        fn vars(home: &str, set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
            let mut map: BTreeMap<String, String> = env(set);
            map.insert("HOME".into(), home.to_string());
            move |k: &str| map.get(k).cloned()
        }
        let from_home = rustc_wrapper(&src, vars(&home, &[])).unwrap();
        assert_eq!(from_home.0, "/bin/kache");
        assert!(from_home.1.ends_with("home/.cargo/config.toml"), "{}", from_home.1);
        assert_eq!(
            rustc_wrapper(&src, vars(&home, &[("RUSTC_WRAPPER", "/bin/other")])),
            Some(("/bin/other".to_string(), "RUSTC_WRAPPER".to_string()))
        );
        assert_eq!(rustc_wrapper(&src, vars(&home, &[("RUSTC_WRAPPER", "")])), None);
        // A config nearer the source wins, and an empty value turns it off.
        fs::create_dir_all(root.join("lab/.cargo")).unwrap();
        fs::write(root.join("lab/.cargo/config.toml"), "[build]\nrustc-wrapper = \"\"\n").unwrap();
        assert_eq!(rustc_wrapper(&src, vars(&home, &[])), None);
        // One without the key leaves it to the next.
        fs::write(root.join("lab/.cargo/config.toml"), "[alias]\nx = \"y\"\n").unwrap();
        assert_eq!(rustc_wrapper(&src, vars(&home, &[])).unwrap().0, "/bin/kache");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_build_log_says_what_was_reused() {
        let cargo = "   Compiling app v0.1.0 (/lab/src/current/crates/apps/app)\n\
                     \x20  Compiling cli v0.1.0 (/lab/src/current/crates/apps/cli)\n\
                     \x20   Finished `release` profile [optimized] target(s) in 41.20s\n";
        let wrapper = ("/bin/kache".to_string(), "/home/.cargo/config.toml".to_string());
        let line = cargo_summary(cargo, Some(&wrapper));
        assert!(line.contains("compiled 2 crates"), "{line}");
        assert!(line.contains("rustc wrapper /bin/kache"), "{line}");
        assert!(cargo_summary("", None).contains("no rustc wrapper"));

        let build = "analyze: 0.4s, 41234 functions (cached)\n\
            lift: 9.1s, 41000 lifted, 0 not lifted\n\
            emit: 6.0s, 39000 owners (120 cut into parts), 1043 files (12 rewritten), 812.3 MB of C, 41000 entries, 900 resumes\n\
            clang (arm64): 12 compiled, 1031 reused, 900 sources in the cold tier, 3.1s keying (dependency scans, hashing, pins), 20.0s compiling, 1.0s archive, 400.0 MB of objects\n";
        assert_eq!(
            image_summary(build),
            "lab: image reuse: analysis cached, emit rewrote 12/1043 C files, clang reused 1031/1043 shards (12 compiled)"
        );
        assert!(image_summary("analyze: 30.0s, 41234 functions\n").contains("analysis recomputed"));
        assert!(image_summary("fake build\n").contains("no phase lines"));
    }

    #[test]
    fn the_log_tail_starts_where_the_step_did() {
        let root = scratch("tail");
        let log = root.join("build.log");
        fs::write(&log, b"before\nafter\n").unwrap();
        assert_eq!(log_since(&log, 7), "after\n");
        assert_eq!(log_since(&root.join("missing"), 0), "");
        let _ = fs::remove_dir_all(root);
    }
}
