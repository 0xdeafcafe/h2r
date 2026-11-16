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
    rejected(&["--arch", "mips"], "expected host, all, arm64, x86_64, tvos or ios");
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

/// Astra's complete synthetic image: one mov-eax-immediate/ret function,
/// padding, and a kernel thunk. No retail image or instruction bytes.
fn tiny_image() -> Vec<u8> {
    fn put(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut image = vec![0; 0x1200];
    image[..4].copy_from_slice(b"XBEH");
    for (offset, value) in [
        (0x104, 0x10000),
        (0x108, 0x1000),
        (0x10c, 0x3000),
        (0x118, 0x10400),
        (0x11c, 2),
        (0x120, 0x10200),
        (0x128, 0x11000 ^ 0xa8fc57ab),
        (0x158, 0x12000 ^ 0x5b6d40b6),
    ] {
        put(&mut image, offset, value);
    }
    for (at, va, name, raw, name_at) in [
        (0x200, 0x11000, &b".text\0"[..], 0x1000, 0x280),
        (0x238, 0x12000, &b".rdata\0"[..], 0x1100, 0x290),
    ] {
        for (offset, value) in [
            (at, if va == 0x11000 { 6 } else { 2 }),
            (at + 4, va),
            (at + 8, 0x100),
            (at + 12, raw),
            (at + 16, 0x100),
            (at + 20, 0x10000 + name_at as u32),
        ] {
            put(&mut image, offset, value);
        }
        image[name_at..name_at + name.len()].copy_from_slice(name);
    }
    image[0x1000..0x1100].fill(0xcc);
    image[0x1000..0x1006].copy_from_slice(&[0xb8, 1, 0, 0, 0, 0xc3]);
    put(&mut image, 0x1100, 0x8000005f);
    image
}

#[test]
fn synthetic_image_can_emit_without_repository_output() {
    let sandbox = Sandbox::new("synthetic-control");
    std::fs::write(sandbox.0.join("image.xbe"), tiny_image()).unwrap();
    let out = sandbox.0.join("external");
    let result = sandbox
        .command()
        .args(["--no-build", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(out.join("src/h2gen.h").is_file());
    assert!(out.join("src/shard_00011.c").is_file());
}

#[cfg(unix)]
#[test]
fn descendant_directory_and_file_symlinks_cannot_write_into_a_repository() {
    for (relative, directory) in [
        ("src", true),
        ("src/h2gen.h", false),
        ("src/h2abi.h", false),
        ("pin", true),
        ("pin/arm64", true),
        ("pin/arm64/cached.o", false),
        ("obj/cached.o", false),
        ("emit-cache-v1", false),
        ("libh2gen-host.dylib", false),
        (".h2-emit.lock", false),
    ] {
        let sandbox = Sandbox::new("descendants");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(sandbox.0.join("repo"))
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(sandbox.0.join("image.xbe"), tiny_image()).unwrap();
        let target = sandbox.0.join("repo/sentinel.h");
        std::fs::write(&target, "sentinel\n").unwrap();
        let target_dir = sandbox.0.join("repo/src-target");
        std::fs::create_dir(&target_dir).unwrap();
        let out = sandbox.0.join("external");
        let alias = out.join(relative);
        std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(if directory { &target_dir } else { &target }, &alias).unwrap();
        let result = sandbox
            .command()
            .args(["--no-build", "--out"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "{relative} bypassed the output guard"
        );
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "sentinel\n",
            "{relative}"
        );
        assert_eq!(
            std::fs::read_dir(&target_dir).unwrap().count(),
            0,
            "{relative}"
        );
        assert!(
            !out.join("src/h2gen_registry.c").exists(),
            "{relative}: generated before rejecting"
        );
    }
}

#[cfg(unix)]
#[test]
fn external_cache_descendants_and_dangling_output_aliases_are_rejected() {
    let sandbox = Sandbox::new("cache-descendants");
    std::fs::write(sandbox.0.join("image.xbe"), tiny_image()).unwrap();
    let out = sandbox.0.join("external");
    let cache = sandbox.0.join("external-cache");
    std::fs::create_dir(&cache).unwrap();
    let target = sandbox.0.join("repo/sentinel.o");
    std::fs::write(&target, "sentinel\n").unwrap();
    std::os::unix::fs::symlink(&target, cache.join("cached.o")).unwrap();
    let result = sandbox
        .command()
        .args(["--no-build", "--out"])
        .arg(&out)
        .env("H2_OBJ_CACHE", &cache)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlinked output entry"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "sentinel\n");
    assert!(!out.join("src").exists());
    std::fs::remove_file(cache.join("cached.o")).unwrap();
    std::fs::create_dir_all(out.join("src")).unwrap();
    let missing = sandbox.0.join("repo/missing.h");
    std::os::unix::fs::symlink(&missing, out.join("src/h2gen.h")).unwrap();
    let result = sandbox
        .command()
        .args(["--no-build", "--allow-repo-output", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("symlinked output entry"));
    assert!(!missing.exists());
}

#[cfg(unix)]
#[test]
fn mutable_generated_files_detach_hard_links_without_modifying_repository_files() {
    use std::os::unix::fs::MetadataExt;
    for relative in [
        "src/h2gen.h",
        "src/h2simd.h",
        "src/h2x87.h",
        "src/h2gen_registry.c",
        "src/shard_00011.c",
        "src/manifest.tsv",
        "emit-cache-v1",
    ] {
        let sandbox = Sandbox::new("hard-linked-output");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(sandbox.0.join("repo"))
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(sandbox.0.join("image.xbe"), tiny_image()).unwrap();
        let sentinel = sandbox.0.join("repo/sentinel.h");
        std::fs::write(&sentinel, "sentinel\n").unwrap();
        let out = sandbox.0.join("external");
        let destination = out.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::hard_link(&sentinel, &destination).unwrap();
        assert_eq!(std::fs::metadata(&sentinel).unwrap().nlink(), 2);
        let result = sandbox
            .command()
            .args(["--no-build", "--out"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{relative}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "sentinel\n",
            "{relative}"
        );
        assert_ne!(
            std::fs::metadata(&sentinel).unwrap().ino(),
            std::fs::metadata(&destination).unwrap().ino(),
            "{relative}"
        );
        assert_ne!(
            std::fs::read(&destination).unwrap(),
            b"sentinel\n",
            "{relative}"
        );
        // The newly detached tree also remains a normal warm emission hit.
        let warm = sandbox
            .command()
            .args(["--no-build", "--out"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(warm.status.success());
        assert!(String::from_utf8_lossy(&warm.stdout).contains(", hit"));
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "sentinel\n");
    }
}
