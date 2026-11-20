//! Running commands: shown before they run, their exit status read
//! directly, never through a pipe or a shell.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

/// A command line as it would be typed, with the environment it adds.
pub fn show(cmd: &Command) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            parts.push(format!("{}={}", k.to_string_lossy(), quote(&v.to_string_lossy())));
        }
    }
    // The directory only when it is not this one (cargo runs from the
    // workspace root, the web checks from web/).
    if let Some(dir) = cmd.get_current_dir()
        && std::env::current_dir().ok().as_deref() != Some(dir)
    {
        parts.insert(0, format!("cd {} &&", quote(&dir.to_string_lossy())));
    }
    parts.push(cmd.get_program().to_string_lossy().into_owned());
    parts.extend(cmd.get_args().map(|a| quote(&a.to_string_lossy())));
    parts.join(" ")
}

fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

pub fn cargo(root: &Path) -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.current_dir(root);
    c
}

/// Runs `cmd` with the terminal; whether it succeeded.
pub fn run(cmd: &mut Command) -> Result<bool> {
    println!("$ {}", show(cmd));
    let status = cmd.status().with_context(|| format!("could not start {}", cmd.get_program().to_string_lossy()))?;
    Ok(status.success())
}

/// Runs `cmd` with stdout and stderr merged into one pipe, echoing each
/// line as it comes when `echo` is set; (succeeded, the output).
pub fn run_merged(mut cmd: Command, echo: bool) -> Result<(bool, String)> {
    if echo {
        println!("$ {}", show(&cmd));
    }
    let (reader, writer) = std::io::pipe()?;
    cmd.stdout(writer.try_clone()?).stderr(writer).stdin(Stdio::null());
    let mut child = cmd.spawn().with_context(|| format!("could not start {}", show(&cmd)))?;
    // The command holds the pipe's write ends until it is dropped; the
    // read below would never see the end otherwise.
    drop(cmd);
    let mut text = String::new();
    let stdout = std::io::stdout();
    for line in BufReader::new(reader).lines() {
        let line = line.unwrap_or_else(|e| format!("(unreadable output: {e})"));
        if echo {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{line}");
        }
        text.push_str(&line);
        text.push('\n');
    }
    let status = child.wait()?;
    Ok((status.success(), text))
}

/// Runs `cmd` for its stdout; stderr goes to the terminal.
pub fn output(cmd: &mut Command) -> Result<(bool, String)> {
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("could not start {}", cmd.get_program().to_string_lossy()))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_a_command_as_typed() {
        let mut c = Command::new("cargo");
        c.args(["nextest", "run", "-E", "not test(/bench|slow/)"]).env("H2_XBE", "disc/default.xbe");
        assert_eq!(show(&c), "H2_XBE=disc/default.xbe cargo nextest run -E 'not test(/bench|slow/)'");
    }
}
