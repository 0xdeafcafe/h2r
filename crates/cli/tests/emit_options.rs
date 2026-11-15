//! Invalid build options fail before parsing an image or starting clang.
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn rejected(options: &[&str], message: &str) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "h2-build-options-{}-{}.xbe",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // Enough for the CLI's magic check, deliberately too short to parse.
    std::fs::write(&path, b"XBEH synthetic").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_h2"))
        .arg("emit")
        .arg(&path)
        .args(options)
        .output()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(error.contains(message), "{error}");
    assert!(
        !error.contains("opening"),
        "validation must precede XBE parsing: {error}"
    );
}

#[test]
fn unknown_architecture_fails_before_parsing_the_image() {
    rejected(&["--arch", "mips"], "expected host, all, arm64 or x86_64");
}

#[test]
fn invalid_parallelism_fails_before_parsing_the_image() {
    for options in [&["--jobs", "0"][..], &["--jobs", "many"], &["--jobs"]] {
        rejected(options, "--jobs needs a positive count");
    }
}

#[test]
fn running_the_entry_requires_a_host_build() {
    let other = if cfg!(target_arch = "x86_64") {
        "arm64"
    } else {
        "x86_64"
    };
    rejected(
        &["--arch", other, "--run-entry"],
        "--run-entry requires the host architecture",
    );
}

struct Sandbox(std::path::PathBuf);
impl Sandbox {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("h2-emit-options-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("repo/.git")).unwrap();
        std::fs::write(path.join("image.xbe"), b"XBEH synthetic").unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_h2"));
        cmd.arg("emit").arg(self.0.join("image.xbe"));
        cmd.env_remove("H2_OBJ_CACHE");
        cmd.current_dir(self.0.join("repo"));
        cmd
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn inside_repository(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("inside a git work tree"), "{message}");
    assert!(message.contains("--allow-repo-output"), "{message}");
}

#[test]
fn explicit_output_and_cache_paths_inside_repository_are_rejected() {
    let sandbox = Sandbox::new("repository");
    inside_repository(sandbox.command().args(["--out", "./generated"]));
    assert!(!sandbox.0.join("repo/generated").exists());
    let external = sandbox.0.join("external");
    inside_repository(
        sandbox
            .command()
            .arg("--out")
            .arg(&external)
            .env("H2_OBJ_CACHE", "./cache"),
    );
    assert!(
        !external.exists(),
        "cache must be checked before creating output"
    );
    assert!(!sandbox.0.join("repo/cache").exists());
}

#[test]
fn a_repository_tmpdir_cannot_bypass_the_default_output_guard() {
    let sandbox = Sandbox::new("tmpdir");
    inside_repository(sandbox.command().env("TMPDIR", sandbox.0.join("repo")));
    assert!(!sandbox.0.join("repo/h2gen").exists());
}

#[cfg(unix)]
#[test]
fn output_and_cache_symlink_aliases_are_rejected() {
    let sandbox = Sandbox::new("aliases");
    let alias = sandbox.0.join("alias");
    std::os::unix::fs::symlink(sandbox.0.join("repo"), &alias).unwrap();
    inside_repository(sandbox.command().arg("--out").arg(alias.join("generated")));
    let external = sandbox.0.join("external");
    inside_repository(
        sandbox
            .command()
            .arg("--out")
            .arg(&external)
            .env("H2_OBJ_CACHE", alias.join("cache")),
    );
    assert!(!external.exists());
    assert!(!sandbox.0.join("repo/generated").exists());
    assert!(!sandbox.0.join("repo/cache").exists());
}

#[test]
fn repository_output_requires_the_explicit_override() {
    let sandbox = Sandbox::new("override");
    let output = sandbox
        .command()
        .args(["--out", "./generated", "--allow-repo-output"])
        .env("H2_OBJ_CACHE", "./cache")
        .output()
        .unwrap();
    // The deliberately incomplete image fails parsing, after taking the lock
    // in the explicitly allowed synthetic checkout. No game code is emitted.
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("inside a git work tree"));
    assert!(sandbox.0.join("repo/generated/.h2-emit.lock").is_file());
}
