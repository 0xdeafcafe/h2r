//! Starting the game on the map being edited: `h2 play` in its own window,
//! with the agent socket on, the editor's map served through a map
//! override, and the level booted straight in.
//!
//! The command (docs/runtime/level-boot.md, docs/editor/scenario-editor.md
//! §5, docs/runtime/agent-api.md "Connecting"):
//!
//! ```text
//! H2_AGENT=<socket> H2_MAP_DIR=<dir with <map>.map> h2 play <disc> --data <data> --level <map>
//! ```
//!
//! `H2_MAP_DIR` wins over packages as a whole-file override (maps.md §8),
//! so the game loads exactly the bytes the editor holds, edits included,
//! without saving a package or touching the data root's `active.toml`.

use std::path::{Path, PathBuf};

/// Where the `h2` binary is: the one given, else `$H2_BIN`, else the `h2`
/// beside the editor's own executable (a checkout's `target/<profile>/h2`
/// after `cargo build -p cli --bin h2`), else cargo's install directory
/// (`~/.cargo/bin/h2`, where `cargo install` puts it), else `h2` on the
/// `PATH`, else the copy the lab host keeps (`docs/runtime/lab.md`).
pub fn find_h2(given: Option<&Path>) -> Option<PathBuf> {
    find_h2_from(given, std::env::current_exe().ok().as_deref())
}

/// [`find_h2`] with the editor's executable passed in.
pub fn find_h2_from(given: Option<&Path>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = given.filter(|p| !p.as_os_str().is_empty()) {
        return Some(p.to_owned());
    }
    if let Some(p) = std::env::var_os("H2_BIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = exe
        .and_then(Path::parent)
        .map(|d| d.join("h2"))
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(p) = cargo_home.map(|c| c.join("bin").join("h2"))
        && p.is_file()
    {
        return Some(p);
    }
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("h2"))
                .find(|p| p.is_file())
        })
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME")?);
            let lab = home.join("Library/Caches/h2recomp/lab/bin/h2");
            lab.is_file().then_some(lab)
        })
}

/// The open disc image, or the extracted disc containing the open maps.
pub(super) fn source_disc(source: &editor_core::MapSource) -> Option<PathBuf> {
    use editor_core::MapSource;
    let maps = match source {
        MapSource::Disc(disc) => return Some(disc.path().to_owned()),
        MapSource::Folder(dir) => dir.as_path(),
        MapSource::Loose(file) => file.parent()?,
    };
    if !maps.file_name()?.eq_ignore_ascii_case("maps") {
        return None;
    }
    let root = maps.parent()?;
    let root = if root.as_os_str().is_empty() {
        Path::new(".")
    } else {
        root
    };
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .any(|entry| {
            entry.file_name().eq_ignore_ascii_case("default.xbe") && entry.path().is_file()
        })
        .then(|| root.to_owned())
}

/// Write the override under the file name `h2 play --level` will request.
/// A package's served name is the document name. Its cache may still
/// identify a launchable level after a rename.
pub(super) fn write_launch_copy(
    session: &scenario_edit::Session,
    disc: &Path,
    data: &Path,
    dir: &Path,
) -> Result<String, String> {
    let document = session.map_name().ok_or("open a map first")?;
    let (cache, scenario) = session.source_identity().ok_or("no map source")?;
    let names = [document, cache, scenario];
    let mut reason = String::new();
    let mut level: Option<String> = None;
    for (i, name) in names.iter().copied().enumerate() {
        if name.is_empty() || names[..i].iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            continue;
        }
        match boot::level_boot::resolve_in(disc, Some(data), name, Default::default()) {
            Ok(request) => {
                if let Some(other) = &level
                    && !request.name.eq_ignore_ascii_case(other)
                {
                    return Err(format!(
                        "{document} identifies two launch levels ({other} and {}). \
                         Save it as a package and load it from the game's map list.",
                        request.name
                    ));
                }
                level = Some(request.name);
                if i == 0 {
                    break;
                }
            }
            Err(e) if reason.is_empty() => {
                reason = e.to_string().lines().next().unwrap_or_default().to_owned();
            }
            Err(_) => {}
        }
    }
    let level = level.ok_or_else(|| {
        format!(
            "cannot launch {document} from the disc or an installed campaign ({reason}). \
             Save the map as a package and load it from the game's map list, or open a map from the disc."
        )
    })?;
    write_copy(session, &level, dir)?;
    Ok(level)
}

/// Write a play copy from the document, including its unsaved edits. Both
/// a launch override and a map.reload request use the same map writer.
pub(super) fn write_copy(
    session: &scenario_edit::Session,
    level: &str,
    dir: &Path,
) -> Result<PathBuf, String> {
    let name = Path::new(level).file_name().ok_or("missing map name")?;
    let name = name.to_str().ok_or("invalid map name")?.trim_end_matches(".map");
    let file = dir.join(format!("{name}.map"));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    scenario_edit::save::save_as(
        session.source().ok_or("no map source")?,
        session.scenario().ok_or("open a map first")?,
        &file,
    )
    .map_err(|e| format!("writing {}: {e}", file.display()))?;
    Ok(file)
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
    #[test]
    fn find_h2_takes_the_binary_beside_the_editor() {
        let dir = std::env::temp_dir().join(format!("h2-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let h2 = dir.join("h2");
        std::fs::write(&h2, b"").unwrap();
        let exe = dir.join("h2-editor");
        if std::env::var_os("H2_BIN").is_some_and(|v| !v.is_empty()) {
            return; // $H2_BIN wins over the editor's directory
        }
        assert_eq!(super::find_h2_from(None, Some(&exe)), Some(h2.clone()));
        let given = dir.join("named-h2");
        assert_eq!(super::find_h2_from(Some(&given), Some(&exe)), Some(given));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use super::*;
    use crate::play::testing::Temp;

    #[test]
    fn the_open_maps_can_supply_the_extracted_disc_root() {
        let t = Temp::new("extracted-root");
        let maps = t.0.join("Maps");
        std::fs::create_dir(&maps).unwrap();
        let folder = editor_core::MapSource::Folder(maps.clone());
        let loose = editor_core::MapSource::Loose(maps.join("invented.map"));
        assert_eq!(source_disc(&folder), None);
        std::fs::write(t.0.join("DEFAULT.XBE"), b"invented").unwrap();
        assert_eq!(source_disc(&folder), Some(t.0.clone()));
        assert_eq!(source_disc(&loose), Some(t.0.clone()));
    }

    #[test]
    fn a_derived_launch_uses_the_resolved_level_for_the_override() {
        let t = Temp::new("derived-launch");
        let maps = t.0.join("disc/maps");
        std::fs::create_dir_all(&maps).unwrap();
        let source = t.map("anchor");
        let mut cache = scenario_edit::MapSource::file(&source);
        cache.bytes = Some(std::fs::read(&source).unwrap().into());
        let mut session = scenario_edit::Session::new();
        session
            .open_derived(cache, "renamed", Some("my-package".into()), "")
            .unwrap();
        let dir = t.0.join("override");
        let prepare = || write_launch_copy(&session, &t.0.join("disc"), &t.0, &dir);

        std::fs::copy(&source, maps.join("Fixture.map")).unwrap();
        assert_eq!(prepare().unwrap(), "Fixture");
        assert!(dir.join("Fixture.map").is_file());
        assert!(!dir.join("renamed.map").exists());

        // A launchable package identity wins over the name in its cache.
        std::fs::copy(&source, maps.join("renamed.map")).unwrap();
        assert_eq!(prepare().unwrap(), "renamed");
        assert!(dir.join("renamed.map").is_file());
    }

    #[test]
    fn a_renamed_map_can_resolve_by_its_cache_scenario() {
        let t = Temp::new("scenario-launch");
        let maps = t.0.join("disc/maps");
        std::fs::create_dir_all(&maps).unwrap();
        let mut bytes = scenario_edit::fixture::fixture();
        let scenario = br"levels\invented\district";
        bytes[0x1bc..0x1bc + scenario.len()].copy_from_slice(scenario);
        std::fs::write(maps.join("slot.map"), &bytes).unwrap();
        let source = t.0.join("private-edit.map");
        std::fs::write(&source, bytes).unwrap();
        let mut session = scenario_edit::Session::new();
        session.open(scenario_edit::MapSource::file(source)).unwrap();

        let dir = t.0.join("override");
        let name = write_launch_copy(&session, &t.0.join("disc"), &t.0, &dir).unwrap();
        assert_eq!(name, "slot");
        assert!(dir.join("slot.map").is_file());

        // The header's short name also exists as a different boot level.
        // Neither cache alias is strong enough to choose between them.
        std::fs::write(maps.join("fixture.map"), scenario_edit::fixture::fixture()).unwrap();
        let why = write_launch_copy(&session, &t.0.join("disc"), &t.0, &dir).unwrap_err();
        assert!(why.contains("two launch levels") && why.contains("slot") && why.contains("fixture"));
        assert!(!dir.join("fixture.map").exists());
    }

    #[test]
    fn an_unlaunchable_map_leaves_the_play_copy_alone() {
        let t = Temp::new("unlaunchable");
        let disc = t.0.join("disc");
        std::fs::create_dir_all(disc.join("maps")).unwrap();
        let mut session = scenario_edit::Session::new();
        session
            .open(scenario_edit::MapSource::file(t.map("invented")))
            .unwrap();
        let dir = t.0.join("override");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("invented.map");
        std::fs::write(&path, b"running copy").unwrap();

        let why = write_launch_copy(&session, &disc, &t.0, &dir).unwrap_err();
        assert!(
            why.contains("invented") && why.contains("game's map list"),
            "{why}"
        );
        assert_eq!(std::fs::read(path).unwrap(), b"running copy");
    }

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
