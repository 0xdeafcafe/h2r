//! Starting the game on the map being edited: `h2 play` in its own window,
//! with the agent socket on, the editor's map served through a map
//! override, and the level booted straight in.
//!
//! The command (docs/runtime/level-boot.md, docs/editor/scenario-editor.md
//! §5, docs/runtime/agent-api.md "Phase 1"):
//!
//! ```text
//! H2_AGENT=<socket> H2_MAP_DIR=<dir with <map>.map> h2 play <disc> --data <data> --level <map>
//! ```
//!
//! `H2_MAP_DIR` wins over packages as a whole-file override (maps.md §8),
//! so the game loads exactly the bytes the editor holds, edits included,
//! without saving a package or touching the data root's `active.toml`.

use std::path::{Path, PathBuf};

/// Where the `h2` binary is: the one given, else `$H2_BIN`, else cargo's
/// install directory (`~/.cargo/bin/h2`, where `cargo install` puts it),
/// else `h2` on the `PATH`.
pub fn find_h2(given: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = given.filter(|p| !p.as_os_str().is_empty()) {
        return Some(p.to_owned());
    }
    if let Some(p) = std::env::var_os("H2_BIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(p) = cargo_home.map(|c| c.join("bin").join("h2"))
        && p.is_file()
    {
        return Some(p);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join("h2"))
            .find(|p| p.is_file())
    })
}

/// What to start.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub h2: PathBuf,
    pub disc: PathBuf,
    pub data: PathBuf,
    /// The level, by its map file name (`ascension`, `03a_oldmombasa`).
    pub level: String,
    /// The agent socket the game opens.
    pub socket: PathBuf,
    /// A directory holding `<level>.map`, the editor's bytes.
    pub map_dir: Option<PathBuf>,
}

impl Launch {
    /// The program, its arguments and the environment it adds.
    pub fn command(&self) -> (PathBuf, Vec<String>, Vec<(String, String)>) {
        let args = vec![
            "play".to_owned(),
            self.disc.display().to_string(),
            "--data".to_owned(),
            self.data.display().to_string(),
            "--level".to_owned(),
            self.level.clone(),
            "--title".to_owned(),
            format!("Halo 2 - {} (h2-editor)", self.level),
        ];
        let mut env = vec![(
            agent_proto::ENV.to_owned(),
            self.socket.display().to_string(),
        )];
        if let Some(d) = &self.map_dir {
            env.push(("H2_MAP_DIR".to_owned(), d.display().to_string()));
        }
        (self.h2.clone(), args, env)
    }

    /// The command as a shell line, for the log and for running by hand.
    pub fn shell_line(&self) -> String {
        let (prog, args, env) = self.command();
        let q = |s: &str| {
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-=:+,".contains(c))
            {
                s.to_owned()
            } else {
                format!("'{}'", s.replace('\'', r"'\''"))
            }
        };
        let mut out: Vec<String> = env.iter().map(|(k, v)| format!("{k}={}", q(v))).collect();
        out.push(q(&prog.display().to_string()));
        out.extend(args.iter().map(|a| q(a)));
        out.join(" ")
    }

    pub fn spawn(&self, log: &Path) -> std::io::Result<std::process::Child> {
        let (prog, args, env) = self.command();
        let out = std::fs::File::create(log)?;
        let err = out.try_clone()?;
        std::process::Command::new(prog)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
    }
}

/// The editor's scratch for a launch: `$TMPDIR/h2-editor-play-<pid>`.
pub fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!("h2-editor-play-{}", std::process::id()))
}

/// The socket a launched game opens: in the agent's cache directory, where
/// [`discover`] finds it (Unix socket paths must stay short, and
/// `$TMPDIR` differs between sessions).
pub fn socket_for_launch(n: u32) -> PathBuf {
    agent_proto::cache_dir().join(format!("editor-{}-{n}.sock", std::process::id()))
}

/// Agent sockets worth offering: `$H2_AGENT`, the default socket, and
/// every socket file in the agent's cache directory, the existing ones
/// first and without repeats.
pub fn discover() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(v) = std::env::var_os(agent_proto::ENV).filter(|v| !v.is_empty()) {
        add(PathBuf::from(v));
    }
    add(agent_proto::default_socket());
    for p in sockets_in(&agent_proto::cache_dir()) {
        add(p);
    }
    out.sort_by_key(|p| !is_socket(p));
    out
}

/// Socket files directly in `dir`, by name.
pub fn sockets_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_socket(p))
        .collect();
    v.sort();
    v
}

fn is_socket(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(p).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(not(unix))]
    {
        let _ = p;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_boots_the_level_with_the_agent_and_the_override() {
        let l = Launch {
            h2: "/home/u/.cargo/bin/h2".into(),
            disc: "/discs/halo 2.iso".into(),
            data: "/tmp/data".into(),
            level: "ascension".into(),
            socket: "/c/editor-1-0.sock".into(),
            map_dir: Some("/tmp/play/maps".into()),
        };
        let (prog, args, env) = l.command();
        assert_eq!(prog, PathBuf::from("/home/u/.cargo/bin/h2"));
        assert_eq!(
            args[..6],
            [
                "play",
                "/discs/halo 2.iso",
                "--data",
                "/tmp/data",
                "--level",
                "ascension"
            ]
        );
        assert!(env.contains(&("H2_AGENT".into(), "/c/editor-1-0.sock".into())));
        assert!(env.contains(&("H2_MAP_DIR".into(), "/tmp/play/maps".into())));
        let line = l.shell_line();
        assert!(
            line.starts_with(
                "H2_AGENT=/c/editor-1-0.sock H2_MAP_DIR=/tmp/play/maps /home/u/.cargo/bin/h2 play '/discs/halo 2.iso'"
            ),
            "{line}"
        );
        let l = Launch { map_dir: None, ..l };
        assert!(!l.command().2.iter().any(|(k, _)| k == "H2_MAP_DIR"));
    }

    #[test]
    fn a_given_h2_wins() {
        assert_eq!(
            find_h2(Some(Path::new("/opt/h2"))),
            Some(PathBuf::from("/opt/h2"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn sockets_are_found_in_a_directory() {
        let dir = std::env::temp_dir().join(format!("h2e-sock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let s = dir.join("a.sock");
        let _l = std::os::unix::net::UnixListener::bind(&s).unwrap();
        std::fs::write(dir.join("not-a-socket.sock"), b"").unwrap();
        assert_eq!(sockets_in(&dir), vec![s]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
