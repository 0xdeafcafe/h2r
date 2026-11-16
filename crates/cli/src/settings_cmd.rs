//! `h2 settings list|get|set`: our settings (`boot::setting_defs`) as the
//! game would see them with this environment, and the one place besides
//! the in-game rows that writes `<data>/settings.toml`
//! (docs/settings-menu.md).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use boot::setting_defs::ALL;
use settings::{Apply, Def, Flag, Kind, Store, Tier};

const USAGE: &str = "usage:
  h2 settings [--data DIR] list [--all]
  h2 settings [--data DIR] get KEY
  h2 settings [--data DIR] set KEY VALUE   (or KEY=VALUE)

Our settings (not the game's own), saved in <data>/settings.toml; --data
defaults to $TMPDIR/h2boot-data, as for `h2 play`. `list` shows each value
in effect for an `h2 play` started from this shell and where it comes from:
flag, environment, settings file or default. --all adds advanced and
diagnostic settings. `set` validates and writes the file, keeping every
other key; a variable set in this environment still wins at launch, and
`h2 play --set KEY=VALUE` sets one for a single run.";

/// `--set KEY=VALUE` from `h2 play`: refused when the key is not one of
/// ours or the value is not one it can hold.
pub fn set_flag(text: &str) -> Result<Flag> {
    let flag = Flag::parse_set(text).map_err(anyhow::Error::msg)?;
    let def = find(&flag.key)?;
    def.parse(&flag.value)
        .map_err(|e| anyhow::anyhow!("--set {}: {e}", def.key))?;
    Ok(flag)
}

fn find(key: &str) -> Result<&'static Def> {
    match ALL.iter().find(|d| d.key == key) {
        Some(d) => Ok(d),
        None => {
            let keys: Vec<&str> = ALL.iter().map(|d| d.key).collect();
            bail!("no setting {key:?}; there are {}", keys.join(", "))
        }
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let mut data = std::env::temp_dir().join("h2boot-data");
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--data" => data = PathBuf::from(it.next().context("--data needs a value")?),
            "-h" | "--help" => bail!("{USAGE}"),
            _ => rest.push(a),
        }
    }
    let env = |k: &str| std::env::var(k).ok();
    print!("{}", command(&data, &rest, &env)?);
    Ok(())
}

/// The command's output, for `data` and the environment `env`.
fn command(data: &Path, args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let path = settings::path_in(data);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut out = String::new();
    match args.as_slice() {
        ["list"] | ["list", "--all"] | [] => {
            let all = args.contains(&"--all");
            let (store, problems) = Store::load(ALL, Some(&path), env, &[]);
            for p in problems {
                eprintln!("settings: {p}");
            }
            for def in store.defs() {
                if all || def.tier == Tier::User {
                    out.push_str(&line(&store, def));
                }
            }
        }
        ["get", key] => {
            let def = find(key)?;
            let (store, problems) = Store::load(ALL, Some(&path), env, &[]);
            for p in problems
                .iter()
                .filter(|p| p.key == Some(def.key) || p.key.is_none())
            {
                eprintln!("settings: {p}");
            }
            out.push_str(&line(&store, def));
            let _ = writeln!(out, "  {}", def.help);
            let _ = writeln!(out, "  values: {}", values(def));
            if let Some(e) = def.env {
                let _ = writeln!(out, "  environment: {e}");
            }
        }
        ["set", key, text] => set(&path, key, text, env, &mut out)?,
        ["set", kv] if kv.contains('=') => {
            let (key, text) = kv.split_once('=').unwrap_or((kv, ""));
            set(&path, key, text, env, &mut out)?
        }
        _ => bail!("{USAGE}"),
    }
    Ok(out)
}

/// Validates `text` for `key` and writes it to the file at `path`.
fn set(
    path: &Path,
    key: &str,
    text: &str,
    env: &dyn Fn(&str) -> Option<String>,
    out: &mut String,
) -> Result<()> {
    let def = find(key)?;
    let v = def
        .parse(text)
        .map_err(|e| anyhow::anyhow!("{}: {e}", def.key))?;
    // The file only: what the environment says does not stop a save.
    let (store, problems) = Store::load(ALL, Some(path), &|_| None, &[]);
    if let Some(p) = problems.iter().find(|p| p.key.is_none()) {
        bail!("{p}");
    }
    let o = store.set(def.key, v).map_err(anyhow::Error::msg)?;
    if let Some(e) = o.not_saved {
        bail!("{}: not saved: {e}", def.key);
    }
    let _ = write!(out, "{} = {}", def.key, def.name(v));
    if !o.changed {
        out.push_str(" (unchanged)");
    }
    let _ = writeln!(out, " in {}", path.display());
    if let Some(e) = def
        .env
        .filter(|e| env(e).is_some_and(|v| !v.trim().is_empty()))
    {
        let _ = writeln!(out, "note: {e} is set here and wins at launch");
    }
    Ok(())
}

/// `graphics.fps  60  Frame Rate (settings file; applies at launch)`.
fn line(store: &Store, def: &Def) -> String {
    let Some(s) = store.state(def.key) else {
        return String::new();
    };
    let mut notes = vec![s.source.to_string()];
    if s.saved != s.value {
        notes.push(format!("saved {}", def.name(s.saved)));
    }
    if def.apply != Apply::Live {
        notes.push("applies at launch".into());
    }
    format!(
        "{:<24} {:<10} {} ({})\n",
        def.key,
        def.name(s.value),
        def.label,
        notes.join("; ")
    )
}

fn values(def: &Def) -> String {
    match &def.kind {
        Kind::Bool => "true or false".into(),
        _ => def.expected(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-settings-cmd-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run_with(
        data: &Path,
        args: &[&str],
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        command(data, &args, env)
    }

    #[test]
    fn set_writes_the_file_and_list_and_get_read_it_back() {
        let data = tmp("rw");
        let none = |_: &str| None;
        let out = run_with(&data, &["set", "graphics.fov", "95"], &none).unwrap();
        assert!(out.starts_with("graphics.fov = 95 in "), "{out}");
        run_with(&data, &["set", "graphics.fps=uncapped"], &none).unwrap();
        let text = std::fs::read_to_string(settings::path_in(&data)).unwrap();
        assert_eq!(text, "[graphics]\nfov = 95\nfps = \"uncapped\"\n");
        let list = run_with(&data, &["list"], &none).unwrap();
        assert!(
            list.contains("graphics.fov             95         Field of View (settings file)"),
            "{list}"
        );
        assert!(list.contains(
            "graphics.fps             uncapped   Frame Rate (settings file; applies at launch)"
        ));
        assert!(list.contains("pad_speaker.enabled      true       Controller Speaker (default)"));
        let get = run_with(&data, &["get", "graphics.render_scale"], &none).unwrap();
        assert!(get.contains("values: a whole number from 1 to 6"), "{get}");
        assert!(get.contains("environment: H2_RENDER_SCALE"));
    }

    #[test]
    fn the_environment_shows_and_does_not_stop_a_save() {
        let data = tmp("env");
        let env = |k: &str| (k == "H2_FOV").then(|| "100".to_string());
        let out = run_with(&data, &["set", "graphics.fov", "80"], &env).unwrap();
        assert!(
            out.contains("note: H2_FOV is set here and wins at launch"),
            "{out}"
        );
        let list = run_with(&data, &["list"], &env).unwrap();
        assert!(
            list.contains("graphics.fov             100        Field of View (H2_FOV; saved 80)"),
            "{list}"
        );
    }

    #[test]
    fn bad_keys_and_values_are_refused_and_write_nothing() {
        let data = tmp("bad");
        let none = |_: &str| None;
        assert!(run_with(&data, &["set", "graphics.fov", "200"], &none).is_err());
        assert!(
            run_with(&data, &["set", "graphics.nope", "1"], &none)
                .unwrap_err()
                .to_string()
                .contains("no setting")
        );
        assert!(run_with(&data, &["frob"], &none).is_err());
        assert!(!settings::path_in(&data).exists());
        assert!(set_flag("graphics.fov=90").is_ok());
        assert!(set_flag("graphics.fov=9000").is_err());
        assert!(set_flag("graphics.fov").is_err());
        assert!(set_flag("nope=1").is_err());
    }
}
