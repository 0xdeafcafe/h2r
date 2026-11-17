//! The app's run-time switches. A UWP app is started by the shell with no
//! environment and no arguments, so each switch is a default here that a
//! flag file in LocalState overrides (put it there with the Device Portal's
//! file explorer, then relaunch). The app turns a switch into the variable
//! the game reads before the game starts (`shell`).

use std::path::Path;

/// The host perf line (`boot::host_perf`, `H2_HOST_PERF`): where frame
/// time goes, every 100 frames or 5 s in `h2uwp.log`. On by default while
/// the console's frame rate is being worked on.
pub const HOST_PERF_DEFAULT: bool = true;
/// A file of this name in LocalState turns the host perf line off...
pub const HOST_PERF_OFF: &str = "host-perf-off";
/// ...and one of this name turns it on (when the default is off).
pub const HOST_PERF_ON: &str = "host-perf-on";

/// Whether the host perf line is on for an app whose LocalState is
/// `local`: the off file wins, then the on file, then the default.
pub fn host_perf(local: &Path) -> bool {
    flag(local, HOST_PERF_DEFAULT, HOST_PERF_ON, HOST_PERF_OFF)
}

/// The agent API over TCP (docs/runtime/xbox.md, "Remote control"): on
/// by default in these dev builds, so an agent on the Mac can drive and
/// profile the game. A file `agent-off` in LocalState turns it off.
pub const AGENT_DEFAULT: bool = true;
pub const AGENT_OFF: &str = "agent-off";
pub const AGENT_ON: &str = "agent-on";
/// The port it listens on, on every interface (`agent_proto::XBOX_PORT`;
/// keep them equal).
pub const AGENT_PORT: u16 = 4720;

/// Whether the agent API listens, for an app whose LocalState is `local`.
pub fn agent(local: &Path) -> bool {
    flag(local, AGENT_DEFAULT, AGENT_ON, AGENT_OFF)
}

/// `H2_AGENT` for the app: every interface, [`AGENT_PORT`].
pub fn agent_endpoint() -> String {
    format!("tcp://0.0.0.0:{AGENT_PORT}")
}

/// A file of this name in LocalState sets the game's environment before it
/// starts: `KEY=VALUE` lines (`H2_LEVEL=03a_oldmombasa` to boot straight
/// into a level, docs/runtime/level-boot.md), `#` comments. Only `H2_*`
/// names; a variable already set wins.
pub const ENV_FILE: &str = "h2env";

/// [`ENV_FILE`]'s variables, and a complaint for each line that isn't one.
pub fn parse_env(text: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut vars = Vec::new();
    let mut bad = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            bad.push(format!("line {}: no `=`: {line}", n + 1));
            continue;
        };
        let k = k.trim().strip_prefix("export ").unwrap_or(k.trim()).trim();
        let name_ok = k.starts_with("H2_")
            && k.len() > 3
            && k.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !name_ok {
            bad.push(format!("line {}: {k} is not an H2_ variable", n + 1));
            continue;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        vars.push((k.to_owned(), v.to_owned()));
    }
    (vars, bad)
}

/// A switch with `default`, turned on by a file `on` in `local` and off by
/// a file `off` (which wins). Only the files' metadata is read: the app
/// container refuses opening directories, not looking at files.
pub fn flag(local: &Path, default: bool, on: &str, off: &str) -> bool {
    if local.join(off).is_file() {
        false
    } else if local.join(on).is_file() {
        true
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_off_file_wins_then_the_on_file_then_the_default() {
        let d = std::env::temp_dir().join(format!("uwp-switches-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(flag(&d, true, "on", "off"));
        assert!(!flag(&d, false, "on", "off"));
        std::fs::write(d.join("on"), b"").unwrap();
        assert!(flag(&d, false, "on", "off"));
        std::fs::write(d.join("off"), b"").unwrap();
        assert!(!flag(&d, true, "on", "off"));
        // A directory of the name is not the flag.
        std::fs::remove_file(d.join("off")).unwrap();
        std::fs::create_dir(d.join("off")).unwrap();
        assert!(flag(&d, false, "on", "off"));
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(
            host_perf(Path::new("/nonexistent/local/state")),
            HOST_PERF_DEFAULT
        );
        assert_eq!(agent(Path::new("/nonexistent/local/state")), AGENT_DEFAULT);
        assert_eq!(agent_endpoint(), "tcp://0.0.0.0:4720");
    }

    #[test]
    fn the_env_file_sets_h2_variables_only() {
        let (vars, bad) = parse_env(
            "\u{feff}# boot into a level\nH2_LEVEL=03a_oldmombasa\n  export H2_DIFFICULTY = heroic \n\nH2_X=\"a b\"\nPATH=/bin\nh2_lower=1\nnonsense\nH2_=1\n",
        );
        assert_eq!(
            vars,
            [
                ("H2_LEVEL".to_owned(), "03a_oldmombasa".to_owned()),
                ("H2_DIFFICULTY".to_owned(), "heroic".to_owned()),
                ("H2_X".to_owned(), "a b".to_owned()),
            ]
        );
        assert_eq!(bad.len(), 4, "{bad:?}");
        assert!(bad[0].starts_with("line 6:") && bad[0].contains("PATH"), "{bad:?}");
    }
}
