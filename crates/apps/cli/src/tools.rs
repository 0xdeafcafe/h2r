//! The offline map, tag and script tools live in the `h2-tools` binary
//! (crates/apps/h2-tools), which links none of the game. `h2 <tool> ...`
//! runs it with the same words and exits with its status
//! (docs/runtime/cli-tools.md).
//!
//! An installed or built `h2` runs the `h2-tools` beside its own executable.
//! Under `cargo run` of this package (`cargo h2`), it runs
//! `cargo run -p h2-tools` instead, so the tools are rebuilt when their
//! code changed rather than an older `h2-tools` in the target directory
//! being run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The commands `h2-tools` runs (crates/apps/h2-tools/src/main.rs).
const TOOLS: &[&str] = &[
    "rules",
    "variants",
    "firefight",
    "forge",
    "package",
    "map",
    "testmap",
    "maps",
    "tags",
    "tag",
    "collision",
    "superbounce",
    "hud",
    "object",
    "strings",
    "effect",
    "lights",
    "shader",
    "hs",
    "extract-shaders",
];

/// How to install the tools beside `h2`, for the error when they are not.
const INSTALL: &str = "install it with `cargo install --locked --path crates/apps/h2-tools` \
     (beside `cargo install --locked --path crates/apps/cli`), or build both with \
     `cargo build --release -p cli -p h2-tools`";

/// Whether `h2 <cmd> <rest>` runs in `h2-tools`. `h2 maps install` runs
/// the game's own installer and `h2 forge looks` reads the disc the way
/// Live's art extractor does, so both stay in `h2`.
pub fn forwards(cmd: &str, rest: &[String]) -> bool {
    let first = rest.first().map(String::as_str);
    match cmd {
        "maps" if first == Some("install") => false,
        "forge" if first == Some("looks") => false,
        _ => TOOLS.contains(&cmd),
    }
}

/// Runs `h2-tools <cmd> <rest>`, replacing this process where the platform
/// can, so its exit status, standard streams and signals are the tool's.
pub fn run(cmd: String, rest: Vec<String>) -> Result<()> {
    let mut words = Vec::with_capacity(rest.len() + 1);
    words.push(cmd);
    words.extend(rest);
    let mut tool = command(&words, &Launch::current()?)?;
    exec(&mut tool)
}

/// How this `h2` was started, which decides where `h2-tools` comes from.
struct Launch {
    /// This executable.
    exe: PathBuf,
    /// Under `cargo run` of this package: cargo, and this package's
    /// manifest directory.
    cargo: Option<(OsString, PathBuf)>,
    /// Built without debug assertions: `cargo run --release`'s profile.
    release: bool,
}

impl Launch {
    fn current() -> Result<Self> {
        let exe = std::env::current_exe().context("finding h2's own path")?;
        let own = Path::new(env!("CARGO_MANIFEST_DIR"));
        // `cargo run` gives the program its package's manifest directory.
        let cargo = match (
            std::env::var_os("CARGO"),
            std::env::var_os("CARGO_MANIFEST_DIR"),
        ) {
            (Some(cargo), Some(dir)) if Path::new(&dir) == own => Some((cargo, own.to_path_buf())),
            _ => None,
        };
        Ok(Self {
            exe,
            cargo,
            release: !cfg!(debug_assertions),
        })
    }
}

/// The process that runs `words` (the tool's name first) in `h2-tools`.
fn command(words: &[String], launch: &Launch) -> Result<Command> {
    if let Some((cargo, manifest_dir)) = &launch.cargo {
        let mut c = Command::new(cargo);
        c.arg("run");
        if launch.release {
            c.arg("--release");
        }
        c.arg("-q")
            .arg("--manifest-path")
            .arg(manifest_dir.join("../h2-tools/Cargo.toml"))
            .args(["--bin", "h2-tools", "--"])
            .args(words);
        return Ok(c);
    }
    let mut c = Command::new(beside(&launch.exe)?);
    c.args(words);
    Ok(c)
}

/// `h2-tools` in the directory of `exe`, or of the file `exe` links to.
fn beside(exe: &Path) -> Result<PathBuf> {
    let name = format!("h2-tools{}", std::env::consts::EXE_SUFFIX);
    let real = exe.canonicalize().ok();
    for dir in [exe.parent(), real.as_deref().and_then(Path::parent)]
        .into_iter()
        .flatten()
    {
        let path = dir.join(&name);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("h2-tools not found next to {}; {INSTALL}", exe.display())
}

#[cfg(unix)]
fn exec(tool: &mut Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = tool.exec();
    Err(err).with_context(|| format!("running {}", tool.get_program().to_string_lossy()))
}

#[cfg(not(unix))]
fn exec(tool: &mut Command) -> Result<()> {
    let status = tool
        .status()
        .with_context(|| format!("running {}", tool.get_program().to_string_lossy()))?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-tools-fwd-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| w.to_string()).collect()
    }

    fn installed(exe: PathBuf) -> Launch {
        Launch {
            exe,
            cargo: None,
            release: true,
        }
    }

    fn args(c: &Command) -> Vec<String> {
        c.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_tools_forward_and_the_game_commands_stay() {
        let split = |rest: &str| {
            rest.split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        for (cmd, rest) in [
            ("firefight", "kit beavercreek"),
            ("rules", "lsp"),
            ("variants", "port a.bin"),
            ("forge", "build doc.h2forge"),
            ("package", "check content"),
            ("maps", ""),
            ("tags", "extract --help"),
            ("tag", "ascension x"),
            ("shader", "--all m.map"),
            ("extract-shaders", "--out d"),
        ] {
            assert!(forwards(cmd, &split(rest)), "{cmd} {rest}");
        }
        for (cmd, rest) in [
            ("maps", "install"),
            ("forge", "looks --json"),
            ("play", ""),
            ("serve", ""),
            ("build", ""),
            ("lighting", "ascension"),
            ("bitmap", "ascension x"),
            ("h1", "convert"),
            ("live", "serve"),
        ] {
            assert!(!forwards(cmd, &split(rest)), "{cmd} {rest}");
        }
    }

    #[test]
    fn the_words_pass_through_to_the_tools_beside_h2() {
        let d = scratch("beside");
        let tools = d.join(format!("h2-tools{}", std::env::consts::EXE_SUFFIX));
        fs::write(&tools, b"").unwrap();
        let w = words(&[
            "firefight",
            "kit",
            "beavercreek",
            "--home",
            "blue",
            "",
            "a b",
            "--",
            "-x",
        ]);
        let c = command(&w, &installed(d.join("h2"))).unwrap();
        assert_eq!(Path::new(c.get_program()), tools);
        assert_eq!(args(&c), w);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn missing_tools_name_the_path_and_how_to_install_them() {
        let d = scratch("missing");
        let exe = d.join("h2");
        let err = command(&words(&["rules", "check"]), &installed(exe.clone()))
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with(&format!("h2-tools not found next to {}", exe.display())),
            "{err}"
        );
        assert!(
            err.contains("cargo install --locked --path crates/apps/h2-tools"),
            "{err}"
        );
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn under_cargo_run_the_tools_are_built_with_the_same_profile() {
        let launch = |release| Launch {
            exe: PathBuf::from("/nowhere/h2"),
            cargo: Some(("/bin/cargo".into(), PathBuf::from("/src/crates/apps/cli"))),
            release,
        };
        let w = words(&["firefight", "kit", "beavercreek"]);
        let c = command(&w, &launch(true)).unwrap();
        assert_eq!(c.get_program(), std::ffi::OsStr::new("/bin/cargo"));
        let manifest = Path::new("/src/crates/apps/cli")
            .join("../h2-tools/Cargo.toml")
            .to_string_lossy()
            .into_owned();
        let mut want = words(&["run", "--release", "-q", "--manifest-path"]);
        want.push(manifest);
        want.extend(words(&["--bin", "h2-tools", "--"]));
        want.extend(w.clone());
        assert_eq!(args(&c), want);
        assert!(!args(&command(&w, &launch(false)).unwrap()).contains(&"--release".to_owned()));
    }

    /// The tool sees exactly the words given, and its exit status comes back.
    #[cfg(unix)]
    #[test]
    fn a_tool_gets_every_word_and_returns_its_status() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("run");
        let tools = d.join("h2-tools");
        let seen = d.join("seen");
        fs::write(
            &tools,
            format!(
                "#!/bin/sh\nfor w in \"$@\"; do printf '%s\\n' \"$w\"; done > '{}'\nexit 7\n",
                seen.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&tools, fs::Permissions::from_mode(0o755)).unwrap();
        let w = words(&["hud", "preview", "x", "--out", "a b"]);
        let status = command(&w, &installed(d.join("h2")))
            .unwrap()
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(7));
        assert_eq!(
            fs::read_to_string(&seen).unwrap(),
            "hud\npreview\nx\n--out\na b\n"
        );
        fs::remove_dir_all(d).unwrap();
    }
}
